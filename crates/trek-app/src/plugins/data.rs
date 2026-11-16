//! The read-only data a plugin's page (or its logic) may ask Trek for by name, each behind its
//! permission (`trek_core::plugins::page::DATA`; the permission is checked before this is
//! asked). What's in memory is answered at once; what reads the disk or runs git is handed back
//! as work for a background thread.

use super::render::thread_value;
use crate::workspace::Workspace;
use gpui_kit::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use trek_core::plugins::{Manifest, Permission};

/// The most threads one answer lists.
const MAX_THREADS: usize = 2000;
/// The days `finished` covers: twelve weeks.
pub const FINISHED_DAYS: usize = 84;

pub enum Answer {
    Now(Value),
    /// Work for a background thread.
    Later(Box<dyn FnOnce() -> Result<Value, String> + Send>),
    /// Trek has no data by that name.
    Unknown,
}

/// The project a page's data is about: the thread in front's, else the most recent project.
fn project(ws: &Workspace) -> Option<PathBuf> {
    ws.focused_thread()
        .and_then(|id| ws.thread(id))
        .and_then(|t| ws.project_dir(t).or_else(|| t.cwd.clone()))
        .filter(|p| !trek_core::paths::is_chat_dir(p))
        .or_else(|| ws.projects.first().map(|p| p.path.clone()))
}

/// The data called `name`, for a plugin whose manifest is `manifest`.
pub fn now(name: &str, manifest: &Manifest, ws: &Entity<Workspace>, cx: &App) -> Answer {
    let ws = ws.read(cx);
    match name {
        "threads" => {
            let usage = manifest.wants(&Permission::UsageRead);
            Answer::Now(Value::Array(
                ws.threads
                    .iter()
                    .filter(|t| t.side_of.is_none() && !t.editor_chat)
                    .take(MAX_THREADS)
                    .map(|t| thread_value(t, ws, true, usage))
                    .collect(),
            ))
        }
        "finished" => {
            let settled: Vec<i64> = ws.threads.iter().filter_map(|t| t.settled_at).collect();
            let now = trek_core::store::now_ms();
            let offset = chrono::Local::now().offset().local_minus_utc() as i64;
            let days =
                trek_core::plugins::data::finished_per_day(&settled, now, FINISHED_DAYS, offset);
            let today = (now / 1000 + offset).div_euclid(86_400);
            let first = (today - FINISHED_DAYS as i64 + 1) * 86_400;
            Answer::Now(json!({
                "total": days.iter().sum::<u32>(),
                "today": days.last().copied().unwrap_or(0),
                "days": days,
                // The first day, as a date in the user's calendar.
                "start": chrono::DateTime::from_timestamp(first, 0)
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default(),
                "working": ws.threads.iter().filter(|t| t.run_state == trek_core::RunState::Working).count(),
                "waiting": ws.threads.iter().filter(|t| t.run_state == trek_core::RunState::NeedsYou).count(),
            }))
        }
        "usage" => {
            // Tokens and cost are usage:read's; a thread's title is threads:read's.
            let titles = manifest.wants(&Permission::ThreadsRead);
            let rows: Vec<Value> = ws
                .threads
                .iter()
                .filter_map(|t| {
                    let spend = ws.live.get(&t.id)?.spend.as_ref()?;
                    Some(usage_row(
                        &t.id,
                        &t.title,
                        t.updated_at,
                        (spend.tokens(), spend.usd()),
                        titles,
                    ))
                })
                .take(MAX_THREADS)
                .collect();
            Answer::Now(json!({
                "tokens": rows.iter().filter_map(|r| r["tokens"].as_u64()).sum::<u64>(),
                "cost": rows.iter().filter_map(|r| r["cost"].as_f64()).sum::<f64>(),
                "threads": rows,
            }))
        }
        "commits" => {
            let Some(dir) = project(ws) else {
                return Answer::Now(Value::Null);
            };
            Answer::Later(Box::new(move || {
                let now = chrono::Utc::now().timestamp();
                Ok(
                    match trek_core::plugins::data::commit_year(&dir, true, now) {
                        Some(y) => json!({
                            "weeks": y.weeks,
                            "total": y.total,
                            "start": y.start,
                            "mine": y.mine,
                        }),
                        None => Value::Null,
                    },
                )
            }))
        }
        "project" => {
            let Some(dir) = project(ws) else {
                return Answer::Now(Value::Null);
            };
            Answer::Later(Box::new(move || {
                Ok(json!({
                    "name": dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    "branch": trek_core::git::read(&dir, &["branch", "--show-current"]).unwrap_or_default(),
                }))
            }))
        }
        "notes" => Answer::Later(Box::new(|| {
            Ok(Value::Array(
                trek_core::notes::list_in(&trek_core::notes::notes_dir())
                    .into_iter()
                    .take(500)
                    .map(|n| {
                        json!({
                            "title": n.title(),
                            "preview": n.preview(),
                            "body": n.body,
                            "modified": n.modified,
                        })
                    })
                    .collect(),
            ))
        })),
        _ => Answer::Unknown,
    }
}

/// One thread's row in `usage`: its id, time, tokens and cost, and its title only for a plugin
/// that may read threads (`titles`).
fn usage_row(
    id: &str,
    title: &str,
    updated: i64,
    (tokens, cost): (u64, f64),
    titles: bool,
) -> Value {
    let mut row = json!({
        "id": id,
        "tokens": tokens,
        "cost": cost,
        "updated": updated,
    });
    if titles {
        row["title"] = json!(title);
    }
    row
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the gpui glob import brings its own `test` attribute.
    use super::usage_row;

    #[test]
    fn usage_names_a_thread_only_for_a_plugin_that_may_read_threads() {
        let row = usage_row("t1", "Fix the login bug", 5, (1200, 0.04), false);
        assert_eq!(row["tokens"], 1200);
        assert!(row.get("title").is_none(), "usage:read alone: {row}");
        let row = usage_row("t1", "Fix the login bug", 5, (1200, 0.04), true);
        assert_eq!(row["title"], "Fix the login bug");
    }
}
