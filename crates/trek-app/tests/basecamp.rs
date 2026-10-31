//! Basecamp: opening and leaving it, the empty range, what needs the user, the totals, the chart
//! (its bars per range) and the tables by project and model, the token usage a turn records for
//! it, and every figure checked by hand against a seeded store.

mod common;

use chrono::{FixedOffset, TimeZone as _};
use common::{Trek, mock, new_project, open, run, store_items};
use gpui_kit::TestAppContext;
use trek_app::basecamp::{Step, Summary, Waiting, chart, tables};
use trek_app::workspace::Route;
use trek_core::basecamp::{Range, ThreadActivity};
use trek_core::settings::ThemeChoice;
use trek_core::store::{Activity, Item, Store, UsageRow, now_ms};
use trek_core::{AgentId, Effort, HandHolding, RunState, TokenUsage};

/// A needs row's id.
fn row(thread: &str) -> gpui_kit::SharedString {
    format!("review-{thread}").into()
}

fn theme(cx: &mut TestAppContext, choice: ThemeChoice) {
    cx.update(|cx| trek_app::apply_theme(choice, None, cx));
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
fn activity(
    store: &Store,
    project: Option<(&str, &str)>,
    agent: AgentId,
    model: Option<&str>,
    turns: &[(i64, bool)],
    usage: &[(i64, Option<&str>, u64)],
) -> ThreadActivity {
    let mut thread = store
        .create_thread(
            None,
            agent.clone(),
            model.map(String::from),
            Effort::Medium,
            HandHolding::Auto,
        )
        .unwrap();
    thread.project_id = project.map(|(id, _)| id.to_string());
    let activity = turns
        .iter()
        .flat_map(|(at, failed)| {
            let end = if *failed {
                Activity::TurnStopped {
                    at: *at,
                    took_secs: 10,
                    failed: true,
                }
            } else {
                Activity::TurnEnd {
                    at: *at,
                    took_secs: 10,
                }
            };
            [Activity::Prompt { at: at - 10_000 }, end]
        })
        .collect();
    let usage = usage
        .iter()
        .map(|(at, model, n)| UsageRow {
            thread_id: thread.id.clone(),
            at: *at,
            agent: agent.clone(),
            model: model.map(String::from),
            tokens: TokenUsage {
                input: *n,
                ..Default::default()
            },
            cost: None,
        })
        .collect();
    ThreadActivity {
        thread,
        project: project.map(|(_, name)| name.to_string()),
        activity,
        usage,
    }
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
            assert!(
                trek.visible(cx, "basecamp-empty"),
                "no activity yet, and a way to start"
            );
            assert!(
                trek.visible(cx, "basecamp-needs"),
                "what needs you is always there"
            );
            assert!(!trek.visible(cx, "basecamp-totals") && !trek.visible(cx, "basecamp-chart"));
        }
        trek.click(cx, "basecamp-new-thread");
        let draft = trek.read(cx, |ws, _| ws.route.clone());
        assert_eq!(
            draft,
            Route::Draft { project: None },
            "a new thread starts in no project"
        );
        // ⌘⇧H opens it with the sidebar folded away too; Esc goes back where it was opened from.
        trek.update(cx, |ws, _| ws.sidebar_collapsed = true);
        trek.press(cx, "secondary-shift-h");
        trek.render(cx);
        assert!(trek.visible(cx, "basecamp"));
        assert!(
            !trek.visible(cx, "open-basecamp"),
            "the sidebar stays folded"
        );
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
            for (title, model) in [
                ("Ship the parser", "mock-deep"),
                ("Fix the flaky test", "mock-swift"),
                ("Read the docs", "mock-swift"),
            ] {
                let t = ws
                    .store
                    .create_thread(
                        Some(&trek.project),
                        mock(),
                        Some(model.into()),
                        Effort::Medium,
                        HandHolding::Auto,
                    )
                    .unwrap();
                let items = vec![
                    Item::User {
                        text: title.into(),
                        images: vec![],
                        at: Some(now - 60_000),
                        resume: None,
                        aside: false,
                    },
                    Item::Assistant {
                        text: "Done.".into(),
                    },
                    Item::TurnEnd {
                        at: now - 1_000,
                        took_secs: 59,
                    },
                ];
                store_items(&ws.store, &t.id, items);
                let tokens = TokenUsage {
                    input: 1_000,
                    output: 400,
                    cache_read: if model == "mock-deep" { 30_000 } else { 5_000 },
                    cache_write: 0,
                };
                ws.store
                    .record_usage(&t.id, now - 1_000, &mock(), Some(model), &tokens, None)
                    .unwrap();
                ids.push(t.id);
            }
            ws.store
                .update_thread(&ids[0], |t| {
                    t.last_seen_at = t.updated_at - 1;
                    t.additions = 372;
                    t.deletions = 15;
                })
                .unwrap();
            ws.store
                .update_thread(&ids[1], |t| t.run_state = RunState::Failed)
                .unwrap();
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
        assert_eq!(
            (
                s.projects[0].threads,
                s.projects[0].turns,
                s.projects[0].tokens,
                s.projects[0].failed
            ),
            (3, 3, r.tokens.total(), 1)
        );
        let models: Vec<(&str, usize, usize, u64, usize)> = s
            .models
            .iter()
            .map(|m| (m.label.as_str(), m.threads, m.turns, m.tokens, m.failed))
            .collect();
        assert_eq!(models.len(), 2, "{models:?}");
        assert_eq!(
            (models[0].1, models[0].2, models[0].3, models[0].4),
            (1, 1, 31_400, 0)
        );
        assert_eq!(
            (models[1].1, models[1].2, models[1].3, models[1].4),
            (2, 2, 2 * 6_400, 1)
        );
        // The mock's models have no price: no cost.
        assert!(!s.projects[0].spend.priced());
        // Today, by the hour: the turns sit in the hour they ended.
        assert_eq!((s.chart.step, s.chart.bars.len()), (Step::Hour, 24));
        assert_eq!(s.chart.bars.iter().map(|b| b.turns).sum::<usize>(), 3);
        let hour = chrono::Timelike::hour(
            &chrono::DateTime::from_timestamp_millis(now - 1_000)
                .unwrap()
                .with_timezone(&chrono::Local),
        ) as usize;
        assert_eq!(s.chart.bars[hour].turns, 3);
        for choice in [ThemeChoice::Paper, ThemeChoice::Night] {
            theme(cx, choice);
            trek.render(cx);
            for id in ["basecamp-totals", "basecamp-chart"] {
                assert!(trek.visible(cx, id), "{id} in {choice:?}");
            }
            assert!(
                trek.visible(cx, ("basecamp-project", 0usize))
                    && trek.visible(cx, ("basecamp-model", 1usize))
            );
            // The failure needs the user, the unread thread waits for review; the read one isn't listed.
            assert!(trek.visible(cx, format!("review-failed-{failed}")));
            assert!(trek.visible(cx, format!("review-done-{done}")));
            assert!(!trek.visible(cx, row(&quiet)));
        }
        // Hovering a bar spells it out.
        let bar = trek
            .bounds(cx, ("basecamp-bar", hour))
            .expect("the bar was drawn");
        trek.window(cx, |window, cx| {
            let at = bar.center();
            window.dispatch_event(
                gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                    position: at,
                    pressed_button: None,
                    modifiers: Default::default(),
                }),
                cx,
            )
        });
        cx.run_until_parked();
        let basecamp = cx.read(|cx| trek.root.read(cx).basecamp.clone());
        assert_eq!(basecamp.read_with(cx, |b, _| b.hovered_bar()), Some(hour));
        let order: Vec<String> = trek.read(cx, |ws, _| {
            ws.ready_for_review().iter().map(|t| t.id.clone()).collect()
        });
        assert_eq!(
            order,
            [failed.clone(), done.clone()],
            "what needs you comes first"
        );
        // Mark all read: the finished one is reviewed; the failure still needs the user.
        trek.click(cx, "basecamp-mark-read");
        trek.render(cx);
        assert!(!trek.visible(cx, row(&done)));
        assert!(trek.visible(cx, row(&failed)));
        // A row opens its thread.
        trek.click(cx, row(&failed));
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Thread(failed)
        );
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
        activity(
            &store,
            Some(("p1", "Trek")),
            claude.clone(),
            Some("claude-opus-5-5"),
            &[(at, false), (at + 60_000, true), (before, false)],
            &[(at, None, 1_000), (before, None, 50)],
        ),
        // Trek again: left on the agent's default, its reports name Sonnet (a dated snapshot too).
        activity(
            &store,
            Some(("p1", "Trek")),
            claude.clone(),
            None,
            &[(at, false)],
            &[
                (at, Some("claude-sonnet-5")),
                (at + 1, Some("claude-sonnet-5-20260101")),
            ]
            .map(|(a, m)| (a, m, 500)),
        ),
        // Website, on Codex.
        activity(
            &store,
            Some(("p2", "Website")),
            AgentId::Codex,
            Some("gpt-5.6-luna"),
            &[(at, false)],
            &[(at, None, 4_000)],
        ),
        // No project.
        activity(
            &store,
            None,
            claude.clone(),
            Some("claude-opus-5-5"),
            &[(at, false)],
            &[(at, None, 10)],
        ),
    ];
    // A sub-agent of the first: its work counts, not as a thread of the user's.
    let mut sub = activity(
        &store,
        Some(("p1", "Trek")),
        claude.clone(),
        Some("claude-opus-5-5"),
        &[(at, false)],
        &[(at, None, 200)],
    );
    sub.thread.parent_id = Some(threads[0].thread.id.clone());
    threads.push(sub);
    // Failed earlier with no record of its turns: one failure, nothing else.
    let mut failed = activity(
        &store,
        Some(("p2", "Website")),
        AgentId::Codex,
        Some("gpt-5.6-luna"),
        &[],
        &[],
    );
    failed.thread.run_state = RunState::Failed;
    failed.thread.updated_at = at;
    threads.push(failed);

    let recap = trek_core::basecamp::Recap::compute(window, at + 120_000, &threads);
    let (projects, models) = tables(&window, &threads);
    let by: Vec<(&str, usize, usize, u64, usize)> = projects
        .iter()
        .map(|r| (r.label.as_str(), r.threads, r.turns, r.tokens, r.failed))
        .collect();
    assert_eq!(
        by,
        [
            ("Website", 1, 1, 4_000, 1),
            ("Trek", 2, 4, 2_200, 1),
            ("No project", 1, 1, 10, 0)
        ]
    );
    assert_eq!(projects[0].project.as_deref(), Some("p2"));
    assert_eq!(projects[2].project, None);
    // Every column adds up to the range's totals.
    let sum = |rows: &[trek_app::basecamp::Row], f: fn(&trek_app::basecamp::Row) -> u64| {
        rows.iter().map(f).sum::<u64>()
    };
    for rows in [&projects, &models] {
        assert_eq!(sum(rows, |r| r.turns as u64), recap.turns as u64);
        assert_eq!(sum(rows, |r| r.tokens), recap.tokens.total());
        assert_eq!(sum(rows, |r| r.failed as u64), recap.failed as u64);
    }
    // Cost by project and by model comes to the same, reports that name no model priced as
    // their thread's (which the recap's own total can't).
    let usd = |rows: &[trek_app::basecamp::Row]| rows.iter().map(|r| r.spend.usd()).sum::<f64>();
    assert!((usd(&projects) - usd(&models)).abs() < 1e-9 && usd(&models) > recap.spend.usd());
    assert_eq!(sum(&projects, |r| r.threads as u64), recap.threads as u64);
    // By model: Opus (three threads' turns, the sub-agent's among them), Sonnet (the snapshot
    // counted as its model), Luna.
    let by: Vec<(&str, Option<&AgentId>, usize, usize, u64, usize)> = models
        .iter()
        .map(|r| {
            (
                r.label.as_str(),
                r.agent.as_ref(),
                r.threads,
                r.turns,
                r.tokens,
                r.failed,
            )
        })
        .collect();
    assert_eq!(
        by,
        [
            ("GPT-5.6 Luna", Some(&AgentId::Codex), 1, 1, 4_000, 1),
            ("Claude Opus 5.5", Some(&claude), 2, 4, 1_210, 1),
            ("Claude Sonnet 5", Some(&claude), 1, 1, 1_000, 0),
        ]
    );
    // Each priced at API prices.
    assert!(
        models
            .iter()
            .all(|m| m.spend.priced() && m.spend.unpriced() == 0 && m.spend.usd() > 0.),
        "{models:?}"
    );
}

#[test]
fn the_chart_is_drawn_in_hours_days_or_weeks_by_range() {
    let store = Store::in_memory().unwrap();
    let tz = FixedOffset::east_opt(2 * 3_600).unwrap();
    // Thursday 8 October 2026, 3:30 PM.
    let now = tz.with_ymd_and_hms(2026, 10, 8, 15, 30, 0).unwrap();
    let ms = |d: i64, h: u32| {
        (tz.with_ymd_and_hms(2026, 10, 8, h, 10, 0).unwrap() - chrono::Duration::days(d))
            .timestamp_millis()
    };
    let threads = [activity(
        &store,
        None,
        mock(),
        None,
        &[
            (ms(0, 14), false),
            (ms(0, 14), false),
            (ms(2, 9), false),
            (ms(41, 9), false),
        ],
        &[(ms(0, 14), None, 700), (ms(2, 9), None, 300)],
    )];
    let turns = |c: &trek_app::basecamp::Chart| c.bars.iter().map(|b| b.turns).collect::<Vec<_>>();
    let ticks = |c: &trek_app::basecamp::Chart| {
        c.ticks
            .iter()
            .map(|(_, l)| l.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };

    // Today: 24 hours, the turns in the 2 PM bar.
    let c = chart(Range::Today, &now, None, &threads);
    assert_eq!((c.step, c.bars.len()), (Step::Hour, 24));
    assert_eq!(
        (
            c.bars[14].turns,
            c.bars[14].tokens,
            turns(&c).iter().sum::<usize>()
        ),
        (2, 700, 2)
    );
    assert_eq!(c.bars[14].label, "2 PM–3 PM");
    assert_eq!(ticks(&c), "12 AM 6 AM Noon 6 PM");

    // This week: Monday to Sunday, a day a bar.
    let c = chart(Range::Week, &now, None, &threads);
    assert_eq!((c.step, c.bars.len()), (Step::Day, 7));
    assert_eq!(turns(&c), [0, 1, 0, 2, 0, 0, 0]);
    assert_eq!(ticks(&c), "Mon Tue Wed Thu Fri Sat Sun");
    assert_eq!(c.bars[3].label, "Thu, Oct 8");

    // All time, with only today's work: still a week of days, never a single day.
    let today_only = [activity(
        &store,
        None,
        mock(),
        None,
        &[(ms(0, 14), false)],
        &[],
    )];
    let c = chart(Range::All, &now, Some(ms(0, 14)), &today_only);
    assert_eq!((c.step, c.bars.len()), (Step::Day, 7));
    assert_eq!(turns(&c), [0, 0, 0, 0, 0, 0, 1]);
    assert_eq!(ticks(&c), "Oct 2 Oct 3 Oct 4 Oct 5 Oct 6 Oct 7 Oct 8");
    let c = chart(Range::All, &now, None, &[]);
    assert_eq!(
        (c.step, c.bars.len()),
        (Step::Day, 7),
        "nothing at all: the last week"
    );

    // All time over six weeks: a day a bar from the first day, dates spread under them.
    let c = chart(Range::All, &now, Some(ms(41, 9)), &threads);
    assert_eq!((c.step, c.bars.len()), (Step::Day, 42));
    assert_eq!(
        (c.bars[0].turns, c.bars[39].turns, c.bars[41].turns),
        (1, 1, 2)
    );
    assert_eq!(c.ticks.len(), 6);
    assert_eq!((c.ticks[0].0, c.ticks[5].0), (0, 41));
    assert_eq!(c.ticks[0].1, "Aug 28");
    assert!(c.ticks.windows(2).all(|w| w[0].0 < w[1].0));

    // A year and more: a week a bar, from the first Monday; years: several weeks a bar.
    let c = chart(Range::All, &now, Some(ms(400, 9)), &threads);
    assert_eq!(c.step, Step::Weeks(1));
    assert_eq!(c.bars.len(), 58);
    assert!(
        c.bars[0].label.starts_with("Week of ") && c.bars[0].label.ends_with(", 2025"),
        "{}",
        c.bars[0].label
    );
    assert_eq!(turns(&c).iter().sum::<usize>(), 4);
    assert_eq!(
        c.bars.last().unwrap().turns,
        3,
        "this week's: Tuesday's and today's"
    );
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
        assert_eq!(
            (
                rows[0].thread_id.as_str(),
                &rows[0].agent,
                rows[0].model.as_deref()
            ),
            (id.as_str(), &mock(), Some("mock-swift"))
        );
        assert!(rows[0].tokens.output > 0);
        // Basecamp counts it.
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        let s = summary(&trek, cx);
        assert_eq!(
            (s.recap.prompts, s.recap.turns, s.recap.tokens.total()),
            (1, 1, rows[0].tokens.total())
        );
        assert_eq!(
            s.models.first().and_then(|m| m.agent.clone()),
            Some(AgentId::Direct("mock".into()))
        );
        assert_eq!(
            s.models.first().map(|m| (m.turns, m.tokens)),
            Some((1, rows[0].tokens.total()))
        );
    });
}

#[test]
fn the_week_the_palette_and_the_clock() {
    run(async |cx| {
        let trek = open(cx);
        // ⌘K finds it.
        trek.press(cx, "secondary-k");
        trek.type_text(cx, "Basecamp");
        trek.press(cx, "enter");
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Basecamp);
        let id = trek.update(cx, |ws, cx| {
            let t = ws
                .store
                .create_thread(
                    Some(&trek.project),
                    mock(),
                    Some("mock-swift".into()),
                    Effort::Medium,
                    HandHolding::Auto,
                )
                .unwrap();
            let at = now_ms() - 1_000;
            store_items(
                &ws.store,
                &t.id,
                vec![
                    Item::User {
                        text: "go".into(),
                        images: vec![],
                        at: Some(at),
                        resume: None,
                        aside: false,
                    },
                    Item::Assistant { text: "ok".into() },
                    Item::TurnEnd { at, took_secs: 1 },
                ],
            );
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
            for (at, model, project) in [
                (now - 60_000, "mock-swift", &trek.project),
                (long_ago, "mock-deep", &website),
            ] {
                let t = ws
                    .store
                    .create_thread(
                        Some(project),
                        mock(),
                        Some(model.into()),
                        Effort::Medium,
                        HandHolding::Auto,
                    )
                    .unwrap();
                let items = vec![
                    Item::User {
                        text: "go".into(),
                        images: vec![],
                        at: Some(at),
                        resume: None,
                        aside: false,
                    },
                    Item::Assistant { text: "ok".into() },
                    Item::TurnEnd {
                        at: at + 30_000,
                        took_secs: 30,
                    },
                ];
                store_items(&ws.store, &t.id, items);
                ws.store
                    .record_usage(
                        &t.id,
                        at + 30_000,
                        &mock(),
                        Some(model),
                        &TokenUsage {
                            input: 1_000,
                            output: 500,
                            cache_read: 0,
                            cache_write: 0,
                        },
                        None,
                    )
                    .unwrap();
            }
            ws.reload(cx);
        });
        let all = summary(&trek, cx);
        let r = &all.recap;
        assert_eq!(
            (
                r.prompts,
                r.threads,
                r.turns,
                r.agent_secs,
                r.tokens.total()
            ),
            (2, 2, 2, 60, 3_000)
        );
        assert_eq!(r.first, Some(long_ago));
        // Six weeks, a day a bar from the first day; both projects in the table.
        assert_eq!((all.chart.step, all.chart.bars.len()), (Step::Day, 43));
        assert_eq!((all.chart.bars[0].turns, all.chart.bars[42].turns), (1, 1));
        assert_eq!(all.projects.len(), 2);
        for choice in [ThemeChoice::Paper, ThemeChoice::Night] {
            theme(cx, choice);
            trek.render(cx);
            assert!(
                trek.visible(cx, "basecamp-totals") && trek.visible(cx, "basecamp-chart"),
                "{choice:?}"
            );
            assert!(trek.visible(cx, ("basecamp-project", 1usize)));
        }
        // Today's leaves the old thread out; ⌘K on Basecamp switches the range.
        trek.press(cx, "secondary-k");
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
        for (prompt, waiting) in [
            ("mock:questions", Waiting::Question),
            ("mock:permission", Waiting::Approval),
        ] {
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
            assert_eq!(
                trek.read(cx, |ws, _| Waiting::of(ws.pending_request(id))),
                *waiting
            );
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
        assert!(!trek.read(cx, |ws, _| {
            ws.ready_for_review().iter().any(|t| t.id == settled)
        }));
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
                    let t = ws
                        .store
                        .create_thread(
                            Some(&trek.project),
                            mock(),
                            None,
                            Effort::Medium,
                            HandHolding::Auto,
                        )
                        .unwrap();
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
        let shown = |trek: &Trek, cx: &mut TestAppContext| {
            ids.iter().filter(|id| trek.visible(cx, row(id))).count()
        };
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
        trek.wait(cx, "the limit to pause the thread", |ws| {
            ws.pause(&thread).is_some() && !ws.turn_running(&thread)
        })
        .await;
        trek.update(cx, |ws, cx| {
            // Another thread resumed at its reset: the "Continue" it sent on its own isn't one of
            // the user's prompts, though its turn counts.
            let resumed = ws
                .store
                .create_thread(
                    Some(&trek.project),
                    mock(),
                    None,
                    Effort::Medium,
                    HandHolding::Auto,
                )
                .unwrap();
            let at = now_ms() - 1_000;
            let continued = Item::User {
                text: trek_core::limit::CONTINUE.into(),
                images: vec![],
                at: Some(at),
                resume: None,
                aside: false,
            };
            store_items(
                &ws.store,
                &resumed.id,
                vec![
                    continued,
                    Item::Assistant {
                        text: "Done.".into(),
                    },
                    Item::TurnEnd { at, took_secs: 1 },
                ],
            );
            ws.store.update_thread(&id, |t| t.last_seen_at = 0).unwrap();
            ws.reload(cx);
            ws.navigate(Route::Basecamp, cx);
        });
        let r = recap(&trek, cx);
        assert_eq!(
            (r.prompts, r.failed),
            (1, 0),
            "paused, not failed; one prompt of the user's"
        );
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
            let at = now_ms().max(Range::Today.window(&chrono::Local::now()).start + 61_000)
                - ago
                - 60_000;
            let t = ws
                .store
                .create_thread(
                    Some(project),
                    mock(),
                    Some("mock-swift".into()),
                    Effort::Medium,
                    HandHolding::Auto,
                )
                .unwrap();
            let items = vec![
                Item::User {
                    text: "go".into(),
                    images: vec![],
                    at: Some(at),
                    resume: None,
                    aside: false,
                },
                Item::Assistant { text: "ok".into() },
                Item::TurnEnd {
                    at: at + 30_000,
                    took_secs: 30,
                },
            ];
            store_items(&ws.store, &t.id, items);
            ws.store
                .record_usage(
                    &t.id,
                    at + 30_000,
                    &mock(),
                    Some("mock-swift"),
                    &TokenUsage {
                        input: 1_000,
                        output: 500,
                        cache_read: 0,
                        cache_write: 0,
                    },
                    None,
                )
                .unwrap();
        }
        ws.reload(cx);
    });
}

#[test]
fn a_narrow_page_keeps_the_names_in_the_tables_readable() {
    run(async |cx| {
        let trek = open(cx);
        worked(&trek, cx, &[(0, &trek.project)]);
        // Beside the sidebar and an open tools panel the page is only a few hundred points wide.
        cx.simulate_window_resize(
            trek.window,
            gpui_kit::size(gpui_kit::px(700.), gpui_kit::px(900.)),
        );
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        trek.render(cx);
        let name = trek
            .bounds(cx, "basecamp-project-name-0")
            .expect("the project's name was drawn");
        assert!(
            name.size.width >= gpui_kit::px(90.),
            "the name keeps its room, the numbers give way: {name:?}"
        );
        let row = trek
            .bounds(cx, ("basecamp-project", 0usize))
            .expect("the row was drawn");
        assert!(
            row.right() <= gpui_kit::px(700.),
            "and the row fits the window: {row:?}"
        );
    });
}

#[test]
fn every_range_is_worked_out_in_one_pass_and_switching_reads_nothing() {
    run(async |cx| {
        let trek = open(cx);
        let website = new_project("website");
        worked(
            &trek,
            cx,
            &[(0, &trek.project.clone()), (42 * 24 * 3_600_000, &website)],
        );
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        assert_eq!(recap(&trek, cx).prompts, 1);
        let basecamp = cx.read(|cx| trek.root.read(cx).basecamp.clone());
        let passes = basecamp.read_with(cx, |b, _| b.passes);
        for (i, range, prompts) in [
            (2usize, Range::All, 2),
            (1, Range::Week, 1),
            (0, Range::Today, 1),
            (2, Range::All, 2),
        ] {
            trek.click(cx, ("basecamp-range", i));
            // Drawn at once from what the one pass worked out: nothing is read again.
            let s = basecamp
                .read_with(cx, |b, _| b.summary().cloned())
                .expect("up to date at once");
            assert_eq!((s.recap.window.range, s.recap.prompts), (range, prompts));
            assert_eq!(
                basecamp.read_with(cx, |b, _| b.passes),
                passes,
                "no pass for {range:?}"
            );
            trek.render(cx);
            assert!(trek.visible(cx, "basecamp-totals"));
        }
        // A turn ends: one pass brings every range up to date, reading only that thread.
        worked(&trek, cx, &[(0, &trek.project.clone())]);
        let all = summary(&trek, cx);
        assert_eq!(all.recap.prompts, 3);
        assert_eq!(basecamp.read_with(cx, |b, _| b.passes), passes + 1);
        trek.click(cx, ("basecamp-range", 0usize));
        assert_eq!(
            basecamp.read_with(cx, |b, _| b.summary().map(|s| s.recap.prompts)),
            Some(2)
        );
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
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(project),
                },
                cx,
            )
        });
        // More work while it's closed: nothing is read until it opens.
        worked(&trek, cx, &[(0, &trek.project.clone())]);
        let basecamp = cx.read(|cx| trek.root.read(cx).basecamp.clone());
        let passes = basecamp.read_with(cx, |b, _| b.passes);
        trek.ws
            .update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        // At once: the numbers it had, while the new ones are worked out off the main thread.
        let (shown, current) = basecamp.read_with(cx, |b, _| {
            (
                b.shown_summary().map(|s| s.recap.prompts),
                b.summary().is_some(),
            )
        });
        assert_eq!((shown, current), (Some(1), false));
        trek.window(cx, |window, cx| {
            gpui_kit::test::TestWindowExt::render_frame(window, cx)
        });
        assert!(
            trek.visible(cx, "basecamp-totals"),
            "drawn before the pass is done"
        );
        assert_eq!(recap(&trek, cx).prompts, 2);
        assert_eq!(basecamp.read_with(cx, |b, _| b.passes), passes + 1);
    });
}

/// A local time in `tz` ("2026-10-28 10:00"), in unix ms.
fn local(tz: &FixedOffset, s: &str) -> i64 {
    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
    tz.from_local_datetime(&naive).unwrap().timestamp_millis()
}

/// A Claude Code session file as Claude Code writes them, with its sub-agents' files beside it:
/// the first an Agent tool's, the rest a workflow's (`subagents/workflows/<run>/`).
fn claude_session(native: &str, lines: &[serde_json::Value], subs: &[serde_json::Value]) {
    let dir = trek_core::paths::home()
        .join(".claude/projects")
        .join("-basecamp-accuracy");
    std::fs::create_dir_all(&dir).unwrap();
    let body = |lines: &[serde_json::Value]| {
        lines
            .iter()
            .map(|l| l.to_string() + "\n")
            .collect::<String>()
    };
    std::fs::write(dir.join(format!("{native}.jsonl")), body(lines)).unwrap();
    if let Some((first, rest)) = subs.split_first() {
        let sub = dir.join(native).join("subagents");
        let workflow = sub.join("workflows").join("wf_1");
        std::fs::create_dir_all(&workflow).unwrap();
        std::fs::write(
            sub.join("agent-a1.jsonl"),
            body(std::slice::from_ref(first)),
        )
        .unwrap();
        std::fs::write(workflow.join("agent-w1.jsonl"), body(rest)).unwrap();
    }
}

fn claude_user(native: &str, text: &str, at: &str) -> serde_json::Value {
    serde_json::json!({ "type": "user", "timestamp": at, "sessionId": native, "isSidechain": false,
        "uuid": format!("u-{at}"), "message": { "role": "user", "content": text } })
}

/// A response's line; `usage` is `[input, output, cache read, cache write, of which an hour]`.
fn claude_reply(
    native: &str,
    id: &str,
    model: &str,
    usage: [u64; 5],
    at: &str,
    done: bool,
) -> serde_json::Value {
    let [input, output, read, write, hour] = usage;
    serde_json::json!({ "type": "assistant", "timestamp": at, "sessionId": native, "isSidechain": false,
        "uuid": format!("a-{id}-{at}"),
        "message": { "id": id, "role": "assistant", "model": model,
            "content": [{ "type": "text", "text": "Done." }],
            "stop_reason": if done { serde_json::Value::from("end_turn") } else { serde_json::Value::Null },
            "usage": { "input_tokens": input, "output_tokens": output, "cache_read_input_tokens": read,
                "cache_creation_input_tokens": write,
                "cache_creation": { "ephemeral_5m_input_tokens": write - hour, "ephemeral_1h_input_tokens": hour } } } })
}

fn forked(mut line: serde_json::Value, from: &str) -> serde_json::Value {
    line["forkedFrom"] = serde_json::json!({ "sessionId": from, "messageUuid": "a-m1" });
    line
}

fn tokens(input: u64, output: u64, cache_read: u64, cache_write: u64) -> TokenUsage {
    TokenUsage {
        input,
        output,
        cache_read,
        cache_write,
    }
}

/// Every figure Basecamp shows, worked out by hand from a store seeded with a week of work in a
/// time zone behind UTC: turns across midnight and across the week's start, a sub-agent, a
/// thread still running, a failed turn, threads the user archived, imported Claude Code
/// sessions (one with a sub-agent of its own and hour-long cache writes, a fork of it, one
/// archived, one the import set aside), and a thread whose agent reported no tokens.
#[test]
fn every_figure_basecamp_shows_adds_up_from_the_store() {
    use trek_core::basecamp::Gathered;
    use trek_core::types::UsageCost;
    let tz = FixedOffset::west_opt(5 * 3600).unwrap();
    let at = |s: &str| local(&tz, s);
    // Wednesday afternoon; the week began on Monday the 26th.
    let now = chrono::DateTime::from_timestamp_millis(at("2026-10-28 15:00"))
        .unwrap()
        .with_timezone(&tz);
    let store = Store::in_memory().unwrap();
    let (alpha, beta) = (new_project("alpha"), new_project("beta"));
    let claude = AgentId::ClaudeCode;
    let opus = "claude-opus-5-5";
    let sonnet = "claude-sonnet-5-5";
    let new = |dir: Option<&std::path::PathBuf>, agent: AgentId, model: Option<&str>| {
        store
            .create_thread(
                dir.map(|d| d.as_path()),
                agent,
                model.map(String::from),
                Effort::High,
                HandHolding::Auto,
            )
            .unwrap()
    };
    let user = |text: &str, at: i64| Item::User {
        text: text.into(),
        images: vec![],
        at: Some(at),
        resume: None,
        aside: false,
    };
    let reply = || Item::Assistant {
        text: "Done.".into(),
    };
    let end = |at: i64, took_secs: u32| Item::TurnEnd { at, took_secs };
    let set = |id: &str, f: &dyn Fn(&mut trek_core::store::Thread)| {
        store.update_thread(id, |t| f(t)).unwrap();
    };

    // 1. Alpha, Opus: a turn from 23:30 last night to 00:30, one this morning, one still running.
    let t1 = new(Some(&alpha), claude.clone(), Some(opus));
    store_items(
        &store,
        &t1.id,
        vec![
            user("Plan the parser", at("2026-10-27 23:30")),
            reply(),
            end(at("2026-10-28 00:30"), 3_600),
            user("Write it", at("2026-10-28 10:00")),
            reply(),
            end(at("2026-10-28 10:10"), 600),
            user("Now the tests", at("2026-10-28 14:00")),
        ],
    );
    store
        .record_usage(
            &t1.id,
            at("2026-10-28 00:30"),
            &claude,
            Some(opus),
            &tokens(1_000, 2_000, 50_000, 4_000),
            Some(UsageCost::reported(1.0)),
        )
        .unwrap();
    store
        .record_usage(
            &t1.id,
            at("2026-10-28 10:10"),
            &claude,
            Some(opus),
            &tokens(500, 1_500, 20_000, 0),
            None,
        )
        .unwrap();
    set(&t1.id, &|t| {
        t.run_state = RunState::Working;
        t.updated_at = at("2026-10-28 14:00");
    });
    // 2. Its sub-agent on Sonnet: its turn and tokens count, it isn't a thread, its brief no prompt.
    let t2 = new(Some(&alpha), claude.clone(), Some(sonnet));
    store_items(
        &store,
        &t2.id,
        vec![
            user("Read the grammar", at("2026-10-28 10:02")),
            reply(),
            end(at("2026-10-28 10:05"), 180),
        ],
    );
    store
        .record_usage(
            &t2.id,
            at("2026-10-28 10:05"),
            &claude,
            Some(sonnet),
            &tokens(100, 200, 0, 0),
            None,
        )
        .unwrap();
    set(&t2.id, &|t| {
        t.parent_id = Some(t1.id.clone());
        t.updated_at = at("2026-10-28 10:05");
    });
    // 3. Beta, Codex: a turn from Sunday 23:50 to Monday 00:20, one last week, one failed today.
    let codex = AgentId::Codex;
    let t3 = new(Some(&beta), codex.clone(), Some("gpt-6-astra"));
    store_items(
        &store,
        &t3.id,
        vec![
            user("Audit the API", at("2026-10-20 09:00")),
            reply(),
            end(at("2026-10-20 09:05"), 300),
            user("And the client", at("2026-10-25 23:50")),
            reply(),
            end(at("2026-10-26 00:20"), 1_800),
            user("Fix what you found", at("2026-10-28 10:58")),
        ],
    );
    store
        .record_stop(&t3.id, at("2026-10-28 11:00"), 120, true)
        .unwrap();
    for (when, used) in [
        ("2026-10-20 09:05", tokens(5_000, 0, 0, 0)),
        ("2026-10-26 00:20", tokens(10_000, 1_000, 90_000, 0)),
    ] {
        store
            .record_usage(&t3.id, at(when), &codex, Some("gpt-6-astra"), &used, None)
            .unwrap();
    }
    set(&t3.id, &|t| {
        t.run_state = RunState::Failed;
        t.updated_at = at("2026-10-28 11:00");
    });
    // 4. Alpha, archived by the user after this morning's turn: the work still counts.
    let t4 = new(Some(&alpha), claude.clone(), Some(opus));
    store_items(
        &store,
        &t4.id,
        vec![
            user("Tidy the README", at("2026-10-28 08:50")),
            reply(),
            end(at("2026-10-28 09:00"), 120),
        ],
    );
    store
        .record_usage(
            &t4.id,
            at("2026-10-28 09:00"),
            &claude,
            Some(opus),
            &tokens(0, 10_000, 0, 0),
            Some(UsageCost::reported(0.25)),
        )
        .unwrap();
    set(&t4.id, &|t| {
        t.archived_at = Some(at("2026-10-28 09:30"));
        t.updated_at = at("2026-10-28 09:00");
    });
    // 5. Alpha, imported from Claude Code (13:00 UTC is 08:00 here): a response written as two
    // lines, its cache written for an hour, and two sub-agents of its own on Sonnet (one in a
    // workflow).
    let imported = |dir: Option<&std::path::PathBuf>, native: &str, last: i64| {
        let t = new(dir, claude.clone(), None);
        set(&t.id, &|t| {
            t.source = trek_core::ThreadSource::ClaudeCode;
            t.native_id = Some(native.to_string());
            t.updated_at = last;
        });
        t
    };
    let n5 = uuid::Uuid::new_v4().to_string();
    claude_session(
        &n5,
        &[
            claude_user(&n5, "Plan the release", "2026-10-28T13:00:00.000Z"),
            claude_reply(
                &n5,
                "m1",
                opus,
                [10, 5, 1_000, 2_000, 2_000],
                "2026-10-28T13:00:59.000Z",
                false,
            ),
            claude_reply(
                &n5,
                "m1",
                opus,
                [10, 100, 1_000, 2_000, 2_000],
                "2026-10-28T13:01:00.000Z",
                true,
            ),
        ],
        &[
            claude_reply(
                &n5,
                "s1",
                sonnet,
                [5, 50, 500, 0, 0],
                "2026-10-28T13:00:30.000Z",
                true,
            ),
            claude_reply(
                &n5,
                "w1",
                sonnet,
                [1, 9, 0, 0, 0],
                "2026-10-28T13:00:40.000Z",
                true,
            ),
        ],
    );
    let t5 = imported(Some(&alpha), &n5, at("2026-10-28 08:01"));
    // 6. A fork of it, continued at noon: the copied conversation is the original's.
    let n6 = uuid::Uuid::new_v4().to_string();
    claude_session(
        &n6,
        &[
            forked(
                claude_user(&n6, "Plan the release", "2026-10-28T13:00:00.000Z"),
                &n5,
            ),
            forked(
                claude_reply(
                    &n6,
                    "m1",
                    opus,
                    [10, 100, 1_000, 2_000, 2_000],
                    "2026-10-28T13:01:00.000Z",
                    true,
                ),
                &n5,
            ),
            claude_user(&n6, "Try it another way", "2026-10-28T17:00:00.000Z"),
            claude_reply(
                &n6,
                "m2",
                opus,
                [1, 10, 100, 0, 0],
                "2026-10-28T17:01:00.000Z",
                true,
            ),
        ],
        &[],
    );
    let t6 = imported(None, &n6, at("2026-10-28 12:01"));
    // 7. Beta, imported and then archived by the user: counted.
    let n7 = uuid::Uuid::new_v4().to_string();
    claude_session(
        &n7,
        &[
            claude_user(&n7, "Check the docs", "2026-10-28T15:00:00.000Z"),
            claude_reply(
                &n7,
                "m7",
                opus,
                [20, 200, 0, 0, 0],
                "2026-10-28T15:02:00.000Z",
                true,
            ),
        ],
        &[],
    );
    let t7 = imported(Some(&beta), &n7, at("2026-10-28 10:02"));
    set(&t7.id, &|t| t.archived_at = Some(at("2026-10-28 10:30")));
    // 8. A session the import set aside (a run in a temp folder): not counted.
    let n8 = uuid::Uuid::new_v4().to_string();
    claude_session(
        &n8,
        &[
            claude_user(&n8, "x", "2026-10-28T15:00:00.000Z"),
            claude_reply(
                &n8,
                "m8",
                opus,
                [0, 999_999, 0, 0, 0],
                "2026-10-28T15:01:00.000Z",
                true,
            ),
        ],
        &[],
    );
    let t8 = imported(None, &n8, at("2026-10-28 10:01"));
    set(&t8.id, &|t| {
        t.archived_at = Some(at("2026-10-28 10:01"));
        t.import_hidden = Some("temp-dir".into());
    });
    // 9. No project, an agent that reported no tokens: half an hour of work.
    let t9 = new(None, AgentId::OpenCode, None);
    store_items(
        &store,
        &t9.id,
        vec![
            user("Rename the module", at("2026-10-28 13:00")),
            reply(),
            end(at("2026-10-28 13:30"), 1_800),
        ],
    );
    set(&t9.id, &|t| t.updated_at = at("2026-10-28 13:30"));

    let g = Gathered::gather(&store, None).unwrap();
    // Read again with nothing changed, it comes to the same.
    let again = Gathered::gather(&store, Some(g.clone())).unwrap();
    assert_eq!(again.read, 0);
    let shown = |range: Range| {
        let s = trek_app::basecamp::summary(&g, range, &now);
        assert_eq!(
            trek_app::basecamp::summary(&again, range, &now),
            s,
            "{range:?} read again"
        );
        s
    };
    // The totals as drawn: threads, turns, tokens, cost, failures, agent time.
    let totals = |s: &Summary| {
        (
            s.recap.threads.to_string(),
            s.recap.turns.to_string(),
            trek_app::workspace::fmt_tokens(s.recap.tokens.total()),
            trek_app::basecamp::cost(&s.spend),
            s.recap.failed.to_string(),
            trek_app::basecamp::agent_time(s.recap.agent_secs),
        )
    };
    let usd = |s: &Summary| s.spend.usd();
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
    // What each report costs at API prices (per million: Opus 5.5 $4 in, $20 out, $0.20 cache
    // reads, $5 five-minute and $8 hour-long writes; Sonnet 5.5 $2, $10, $0.20; GPT-6 Astra
    // $10, $50, $1 cached). Claude Code's own figures stand as recorded.
    let t1_morning = (500. * 4. + 1_500. * 20. + 20_000. * 0.2) / 1e6;
    let t2_turn = (100. * 2. + 200. * 10.) / 1e6;
    let t5_reply = (10. * 4. + 100. * 20. + 1_000. * 0.2 + 2_000. * 8.) / 1e6;
    let t5_sub = (5. * 2. + 50. * 10. + 500. * 0.2 + 2. + 9. * 10.) / 1e6;
    let t6_reply = (4. + 10. * 20. + 100. * 0.2) / 1e6;
    let t7_reply = (20. * 4. + 200. * 20.) / 1e6;
    let t3_monday = (10_000. * 10. + 1_000. * 50. + 90_000.) / 1e6;
    let t3_last_week = 5_000. * 10. / 1e6;

    // Today. Threads: 1, 3, 4, 5, 6, 7, 9 (the sub-agent isn't one, the set-aside session isn't
    // counted). Turns: two of 1's (00:30 and 10:10), the sub-agent's, 3's failed one, 4's, 5's,
    // the fork's own, 7's and 9's. Agent time: 1's night turn from midnight only (30 of its 60
    // minutes), the rest whole: 1800 + 600 + 180 + 120 + 120 + 60 + 60 + 120 + 1800 = 4860 s.
    // Tokens: 57,000 + 22,000 + 300 + 10,000 + 3,675 (5's reply and its sub-agents') + 111 (the
    // fork's own) + 220.
    let today = shown(Range::Today);
    assert_eq!(
        totals(&today),
        (
            "7".into(),
            "9".into(),
            "93K".into(),
            "$1.31".into(),
            "1".into(),
            "1h 21m".into()
        )
    );
    assert_eq!(today.recap.tokens.total(), 93_306);
    assert_eq!(today.recap.agent_secs, 4_860);
    let today_usd = 1.0 + t1_morning + t2_turn + 0.25 + t5_reply + t5_sub + t6_reply + t7_reply;
    assert!(
        close(usd(&today), today_usd),
        "{} vs {today_usd}",
        usd(&today)
    );
    assert_eq!(today.spend.unpriced(), 0, "every report has a price");
    // Prompts: 1's two today (the night one was yesterday's), 3's, 4's, 5's, the fork's own, 7's, 9's.
    assert_eq!(today.recap.prompts, 8);
    // The chart: the turns in the hour they ended, adding up to the total.
    assert_eq!((today.chart.step, today.chart.bars.len()), (Step::Hour, 24));
    let by_hour: Vec<(usize, usize)> = today
        .chart
        .bars
        .iter()
        .enumerate()
        .filter(|(_, b)| b.turns > 0)
        .map(|(i, b)| (i, b.turns))
        .collect();
    assert_eq!(
        by_hour,
        [(0, 1), (8, 1), (9, 1), (10, 3), (11, 1), (12, 1), (13, 1)]
    );
    assert_eq!(
        today.chart.bars[10].line(),
        "10 AM–11 AM · 3 turns · 23K tokens"
    );
    // By project: alpha (1 with its sub-agent, 4, 5), beta (3, 7), no project (the fork, 9).
    let rows =
        |rows: &[trek_app::basecamp::Row]| -> Vec<(String, usize, usize, String, String, usize)> {
            rows.iter()
                .map(|r| {
                    (
                        // Projects are named for their folders ("alpha-3").
                        match r.project {
                            Some(_) => r.label.rsplit_once('-').map_or(&*r.label, |(n, _)| n),
                            None => &r.label,
                        }
                        .to_string(),
                        r.threads,
                        r.turns,
                        trek_app::workspace::fmt_tokens(r.tokens),
                        trek_app::basecamp::cost(&r.spend),
                        r.failed,
                    )
                })
                .collect()
        };
    let row = |l: &str, th: usize, tu: usize, tok: &str, c: &str, f: usize| {
        (l.to_string(), th, tu, tok.to_string(), c.to_string(), f)
    };
    assert_eq!(
        rows(&today.projects),
        [
            row("alpha", 3, 5, "93K", "$1.31", 0),
            row("beta", 2, 2, "220", "< $0.01", 1),
            row("No project", 2, 2, "111", "< $0.01", 0),
        ]
    );
    // By model: Opus (1, 4, 5, the fork, 7), Sonnet (the sub-agent's turn; 5's sub-agent's
    // tokens, so 5 counts under it too), then the two that reported no tokens.
    assert_eq!(
        rows(&today.models),
        [
            row("Claude Opus 5.5", 5, 6, "92K", "$1.31", 0),
            row("Claude Sonnet 5.5", 1, 1, "865", "< $0.01", 0),
            row("GPT-6 Astra", 1, 1, "0", "—", 1),
            row("OpenCode", 1, 1, "0", "—", 0),
        ]
    );
    // Every column adds up to the totals (threads aside in the model table, where a thread that
    // used two models counts under each).
    for table in [&today.projects, &today.models] {
        let tokens: u64 = table.iter().map(|r| r.tokens).sum();
        let turns: usize = table.iter().map(|r| r.turns).sum();
        let failed: usize = table.iter().map(|r| r.failed).sum();
        assert_eq!(
            (tokens, turns, failed),
            (
                today.recap.tokens.total(),
                today.recap.turns,
                today.recap.failed
            )
        );
    }

    // This week adds Monday's 00:20 turn (20 of its 30 minutes ran this week) and its tokens,
    // and all of 1's night turn: 4,860 + 1,800 + 1,200 = 7,860 s.
    let week = shown(Range::Week);
    assert_eq!(
        totals(&week),
        (
            "7".into(),
            "10".into(),
            "194K".into(),
            "$1.55".into(),
            "1".into(),
            "2h 11m".into()
        )
    );
    assert_eq!(week.recap.agent_secs, 7_860);
    assert!(close(usd(&week), today_usd + t3_monday));
    let by_day: Vec<usize> = week.chart.bars.iter().map(|b| b.turns).collect();
    assert_eq!(by_day, [1, 0, 9, 0, 0, 0, 0], "Monday to Sunday");

    // All time runs from last Tuesday's first prompt, a day a bar: all of Monday's turn, and the
    // one last week.
    let all = shown(Range::All);
    assert_eq!(
        totals(&all),
        (
            "7".into(),
            "11".into(),
            "199K".into(),
            "$1.60".into(),
            "1".into(),
            "2h 26m".into()
        )
    );
    assert_eq!(all.recap.agent_secs, 8_760);
    assert!(close(usd(&all), today_usd + t3_monday + t3_last_week));
    assert_eq!(all.recap.window.start, at("2026-10-20 00:00"));
    assert_eq!((all.chart.step, all.chart.bars.len()), (Step::Day, 9));
    assert_eq!(
        all.chart.bars.iter().map(|b| b.turns).collect::<Vec<_>>(),
        [1, 0, 0, 0, 0, 0, 1, 0, 9]
    );
    assert_eq!(all.chart.bars[0].label, "Tue, Oct 20");

    // The imported thread's own cost, as its header shows it: its history, sub-agent included,
    // hour-long cache writes at their rate.
    let t5 = store.thread(&t5.id).unwrap().unwrap();
    let history = trek_core::basecamp::history_usage(&store, &t5, true);
    assert_eq!(history.len(), 3);
    assert!(close(
        trek_core::store::UsageRow::spend(&history).usd(),
        t5_reply + t5_sub
    ));
    // Kept by the read: the main thread's quick look finds the same.
    assert_eq!(
        trek_core::basecamp::history_usage(&store, &t5, false),
        history
    );
    let _ = (t6, t8);
}

#[test]
fn token_counts_are_rounded_before_they_are_named() {
    let fmt = trek_app::workspace::fmt_tokens;
    let cases = [
        (999, "999"),
        (1_000, "1K"),
        (93_296, "93K"),
        (999_499, "999K"),
        // Not "1000K", nor "2.0M" for 1.96 million.
        (999_600, "1M"),
        (1_000_000, "1M"),
        (1_250_000, "1.3M"),
        (1_960_000, "2M"),
        (2_959_600, "3M"),
        (12_340_000, "12.3M"),
        (999_960_000, "1B"),
        (3_260_018_742, "3.3B"),
    ];
    for (n, shown) in cases {
        assert_eq!(fmt(n), shown, "{n}");
    }
}
