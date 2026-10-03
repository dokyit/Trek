//! Basecamp: a recap of the work done today or this week. What was asked (prompts), how long
//! the agents worked (turns), which models and projects it went into, and the tokens it took.
//!
//! Everything comes from the store: transcripts' prompts and turn ends, and the token usage
//! agents report as turns end. Imported threads that weren't continued here are read from the
//! agents' own files (and cached). Tokens are only counted where they were reported; the recap
//! says what it knows and nothing more.

use crate::catalog;
use crate::import;
use crate::store::{Activity, Store, Thread, UsageRow};
use crate::types::{AgentId, RunState, ThreadSource, TokenUsage};
use chrono::{DateTime, Datelike as _, Duration, NaiveDate, TimeZone};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

/// The stretch of time a recap covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Range {
    #[default]
    Today,
    /// Since Monday.
    Week,
}

impl Range {
    pub fn label(self) -> &'static str {
        match self {
            Range::Today => "Today",
            Range::Week => "This week",
        }
    }

    /// The calendar day or week (from Monday) holding `now`, in `now`'s time zone, cut into
    /// buckets for the elevation profile: hours for a day, three hours for a week.
    pub fn window<Tz: TimeZone>(self, now: &DateTime<Tz>) -> Window {
        let today = now.date_naive();
        let tz = now.timezone();
        let (first, days, bucket_hours) = match self {
            Range::Today => (today, 1, 1),
            Range::Week => (today - Duration::days(today.weekday().num_days_from_monday() as i64), 7, 3),
        };
        let start = midnight(&tz, first);
        let end = midnight(&tz, first + Duration::days(days));
        Window { range: self, start, end, bucket_ms: bucket_hours * 3_600_000 }
    }
}

/// The first instant of `day` in `tz`. Where a clock change skips midnight, the day starts
/// when the clock comes back (the next hour that exists).
fn midnight<Tz: TimeZone>(tz: &Tz, day: NaiveDate) -> i64 {
    let at = day.and_hms_opt(0, 0, 0).expect("midnight");
    (0..3)
        .find_map(|h| tz.from_local_datetime(&(at + Duration::hours(h))).earliest())
        .map(|d| d.timestamp_millis())
        .unwrap_or_else(|| at.and_utc().timestamp_millis())
}

/// The span a recap covers, in unix ms (`end` is exclusive and may lie ahead of now).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub range: Range,
    pub start: i64,
    pub end: i64,
    pub bucket_ms: i64,
}

impl Window {
    pub fn buckets(&self) -> usize {
        ((self.end - self.start + self.bucket_ms - 1) / self.bucket_ms).max(1) as usize
    }

    pub fn contains(&self, at: i64) -> bool {
        (self.start..self.end).contains(&at)
    }

    fn bucket(&self, at: i64) -> usize {
        (((at - self.start) / self.bucket_ms).max(0) as usize).min(self.buckets() - 1)
    }
}

/// One thread's part in a recap: its prompts and turn ends in the window, and the tokens it
/// reported there.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadActivity {
    pub thread: Thread,
    pub project: Option<String>,
    pub activity: Vec<Activity>,
    pub usage: Vec<UsageRow>,
}

/// What an imported thread's own history holds for a window.
type Imported = Arc<(Vec<Activity>, Vec<UsageRow>)>;
/// By (source, session, window start, read until, with activity): the thread's `updated_at`
/// when it was read (0 when what was read can't change any more), and what it held.
type ImportedKey = (String, String, i64, i64, bool);
static IMPORTED: LazyLock<Mutex<HashMap<ImportedKey, (i64, Imported)>>> = LazyLock::new(Default::default);

/// Everything a recap of `window` needs, read from the store and (for imported threads not
/// continued here) the agents' files. Slow: run it off the main thread.
pub fn gather(store: &Store, window: &Window) -> anyhow::Result<Vec<ThreadActivity>> {
    let threads = store.threads_since(window.start)?;
    let names: HashMap<String, String> = store.projects()?.into_iter().map(|p| (p.id, p.name)).collect();
    let ids: Vec<String> = threads.iter().map(|t| t.id.clone()).collect();
    let stored = store.with_transcripts(&ids)?;
    let recorded_from = store.first_usage(&ids)?;
    let mut activity: HashMap<String, Vec<Activity>> = HashMap::new();
    for (thread, a) in store.activity_between(window.start, window.end)? {
        activity.entry(thread).or_default().push(a);
    }
    let mut usage: HashMap<String, Vec<UsageRow>> = HashMap::new();
    for row in store.usage_between(window.start, window.end)? {
        usage.entry(row.thread_id.clone()).or_default().push(row);
    }
    Ok(threads
        .into_iter()
        .map(|t| {
            let (activity, usage) = match (&t.native_id, t.source) {
                (Some(native), source) if source != ThreadSource::Trek && !stored.contains(&t.id) => {
                    let found = imported(&t, source, native, window, window.end, true);
                    (found.0.clone(), found.1.clone())
                }
                // An imported thread continued here, in the same session: the agent's history
                // has what it used until Trek recorded its first turn, Trek's rows the rest.
                (Some(native), source) if source != ThreadSource::Trek => {
                    let mut usage = usage.remove(&t.id).unwrap_or_default();
                    let until = recorded_from.get(&t.id).map_or(window.end, |at| (*at).min(window.end));
                    if until > window.start {
                        usage.splice(0..0, imported(&t, source, native, window, until, false).1.iter().cloned());
                    }
                    (activity.remove(&t.id).unwrap_or_default(), usage)
                }
                _ => (activity.remove(&t.id).unwrap_or_default(), usage.remove(&t.id).unwrap_or_default()),
            };
            let project = t.project_id.as_ref().and_then(|p| names.get(p).cloned());
            ThreadActivity { thread: t, project, activity, usage }
        })
        .collect())
}

/// An imported thread's tokens in `window` up to `until`, from its agent's history, and its
/// prompts and turns there when `with_activity`. Read again only once the thread has moved on
/// since (what lies before `until` < the window's end can't).
fn imported(t: &Thread, source: ThreadSource, native: &str, window: &Window, until: i64, with_activity: bool) -> Imported {
    let key = (source.key().to_string(), native.to_string(), window.start, until, with_activity);
    let stamp = if until < window.end { 0 } else { t.updated_at };
    if let Some((at, found)) = IMPORTED.lock().expect("basecamp cache").get(&key)
        && *at == stamp
    {
        return found.clone();
    }
    let items = if with_activity {
        import::load_transcript(source, native).unwrap_or_else(|e| {
            tracing::debug!("basecamp: {} {native}: {e:#}", source.key());
            vec![]
        })
    } else {
        vec![]
    };
    let activity: Vec<Activity> = items
        .iter()
        .filter_map(|i| match i {
            crate::store::Item::User { at: Some(at), aside: false, .. } => Some(Activity::Prompt { at: *at }),
            crate::store::Item::TurnEnd { at, took_secs } => Some(Activity::TurnEnd { at: *at, took_secs: *took_secs }),
            _ => None,
        })
        .filter(|a| window.contains(a.at()))
        .collect();
    let agent = source.agent().unwrap_or_else(|| t.agent.clone());
    let usage = import::load_usage(source, native, window.start, until)
        .into_iter()
        .map(|(at, model, tokens)| UsageRow { thread_id: t.id.clone(), at, agent: agent.clone(), model: model.or_else(|| t.model.clone()), tokens })
        .collect();
    let found = Arc::new((activity, usage));
    IMPORTED.lock().expect("basecamp cache").insert(key, (stamp, found.clone()));
    found
}

/// A project's part in a recap.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectShare {
    pub id: String,
    pub name: String,
    pub prompts: usize,
    pub tokens: u64,
}

/// A model's part in a recap: the tokens it reported and the turns it took (each turn on the
/// model the agent reported using for it, else the thread's).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelShare {
    pub agent: AgentId,
    /// `None`: the agent's default, unnamed.
    pub model: Option<String>,
    pub tokens: u64,
    pub turns: usize,
}

/// One stretch of the elevation profile.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bucket {
    pub start: i64,
    pub prompts: usize,
    /// Agent time spent in this stretch, from the turns that overlap it.
    pub agent_secs: u64,
    pub tokens: u64,
}

/// What a recap says, computed once per change (never per frame).
#[derive(Debug, Clone, PartialEq)]
pub struct Recap {
    pub window: Window,
    pub now: i64,
    pub prompts: usize,
    /// Threads with a prompt or a finished turn in the window.
    pub threads: usize,
    pub turns: usize,
    pub agent_secs: u64,
    /// Tokens reported in the window.
    pub tokens: TokenUsage,
    /// Of `threads`, the ones that reported tokens.
    pub threads_with_tokens: usize,
    /// Most prompts first.
    pub projects: Vec<ProjectShare>,
    /// Most tokens first (most turns, where no tokens were reported).
    pub models: Vec<ModelShare>,
    /// Turns in the window that failed (for threads Trek has no record of failed turns for: one
    /// when the thread failed in the window).
    pub failed: usize,
    pub buckets: Vec<Bucket>,
}

/// Comparable model ids: dated snapshots (`claude-haiku-4-5-20251001`) count as their model.
pub fn model_key(model: &str) -> String {
    let m = model.trim().to_lowercase();
    match m.rsplit_once('-') {
        Some((base, date)) if date.len() == 8 && date.chars().all(|c| c.is_ascii_digit()) => base.to_string(),
        _ => m,
    }
}

impl Recap {
    pub fn compute(window: Window, now: i64, threads: &[ThreadActivity]) -> Recap {
        let mut buckets: Vec<Bucket> = (0..window.buckets()).map(|i| Bucket { start: window.start + i as i64 * window.bucket_ms, ..Default::default() }).collect();
        let mut recap = Recap {
            window,
            now,
            prompts: 0,
            threads: 0,
            turns: 0,
            agent_secs: 0,
            tokens: TokenUsage::default(),
            threads_with_tokens: 0,
            projects: vec![],
            models: vec![],
            failed: 0,
            buckets: vec![],
        };
        let mut projects: Vec<ProjectShare> = vec![];
        let mut models: Vec<ModelShare> = vec![];
        for t in threads {
            let mut prompts = 0;
            let mut turns = 0;
            let mut failed = 0;
            let usage: Vec<&UsageRow> = t.usage.iter().filter(|u| window.contains(u.at)).collect();
            // Turns no report names a model for go to the thread's, or to the one its reports
            // name most (a thread left on the agent's default).
            let thread_model = t.thread.model.clone().or_else(|| busiest_model(&usage, i64::MIN, i64::MAX));
            for a in t.activity.iter().filter(|a| window.contains(a.at())) {
                match *a {
                    Activity::Prompt { at } => {
                        prompts += 1;
                        buckets[window.bucket(at)].prompts += 1;
                    }
                    Activity::TurnEnd { at, took_secs } | Activity::TurnStopped { at, took_secs, .. } => {
                        turns += 1;
                        recap.agent_secs += took_secs as u64;
                        let began = at - took_secs as i64 * 1000;
                        spread(&mut buckets, &window, began, at);
                        // Reports land as the turn ends (or during it, in agents' own histories).
                        let model = busiest_model(&usage, began - 1_000, at + 5_000).or_else(|| thread_model.clone());
                        let i = model_at(&mut models, &t.thread.agent, model.as_deref());
                        models[i].turns += 1;
                        if matches!(a, Activity::TurnStopped { failed: true, .. }) {
                            failed += 1;
                        }
                    }
                }
            }
            if failed == 0 && t.thread.run_state == RunState::Failed && window.contains(t.thread.updated_at) {
                failed = 1;
            }
            recap.failed += failed;
            if prompts == 0 && turns == 0 && usage.is_empty() {
                continue;
            }
            // A sub-agent's work counts (its turns and tokens, under its own model), but it isn't
            // a thread of the user's.
            let own = t.thread.parent_id.is_none();
            if own {
                recap.threads += 1;
            }
            recap.prompts += prompts;
            recap.turns += turns;
            let mut tokens = 0;
            for u in &usage {
                recap.tokens.add(&u.tokens);
                tokens += u.tokens.total();
                buckets[window.bucket(u.at)].tokens += u.tokens.total();
                let i = model_at(&mut models, &u.agent, u.model.as_deref());
                models[i].tokens += u.tokens.total();
            }
            if tokens > 0 && own {
                recap.threads_with_tokens += 1;
            }
            if let (Some(id), Some(name)) = (&t.thread.project_id, &t.project) {
                match projects.iter_mut().find(|p| &p.id == id) {
                    Some(p) => {
                        p.prompts += prompts;
                        p.tokens += tokens;
                    }
                    None => projects.push(ProjectShare { id: id.clone(), name: name.clone(), prompts, tokens }),
                }
            }
        }
        projects.retain(|p| p.prompts > 0 || p.tokens > 0);
        projects.sort_by(|a, b| b.prompts.cmp(&a.prompts).then(b.tokens.cmp(&a.tokens)).then(a.name.cmp(&b.name)));
        models.retain(|m| m.tokens > 0 || m.turns > 0);
        models.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(b.turns.cmp(&a.turns)));
        recap.projects = projects;
        recap.models = models;
        recap.buckets = buckets;
        recap
    }

    /// Nothing happened in the window.
    pub fn is_empty(&self) -> bool {
        self.threads == 0
    }

    /// How high the profile stands over bucket `i`: agent minutes when turns were timed, else
    /// prompts.
    pub fn elevation(&self, i: usize) -> f32 {
        let Some(b) = self.buckets.get(i) else { return 0. };
        if self.agent_secs > 0 { b.agent_secs as f32 / 60. } else { b.prompts as f32 }
    }

    /// The busiest stretch, if anything happened.
    pub fn peak(&self) -> Option<usize> {
        (0..self.buckets.len()).filter(|i| self.elevation(*i) > 0.).max_by(|a, b| self.elevation(*a).total_cmp(&self.elevation(*b)).then(b.cmp(a)))
    }

    /// The bucket `now` falls in, while the window is current.
    pub fn now_bucket(&self) -> Option<usize> {
        self.window.contains(self.now).then(|| self.window.bucket(self.now))
    }

    /// The model that carried the most: by tokens where they were reported, else by turns.
    pub fn best_model(&self) -> Option<&ModelShare> {
        self.models.first()
    }

    /// Share of the reported tokens `m` used, in percent. 100 only when it used them all, and
    /// never 0 when it used some: "100%, ahead of …" would contradict itself.
    pub fn token_share(&self, m: &ModelShare) -> Option<u32> {
        let total: u64 = self.models.iter().map(|m| m.tokens).sum();
        if total == 0 || m.tokens == 0 {
            return None;
        }
        if m.tokens >= total {
            return Some(100);
        }
        Some((((m.tokens as f64 / total as f64) * 100.).round() as u32).clamp(1, 99))
    }

    /// Tokens weren't reported for every thread that worked.
    pub fn tokens_partial(&self) -> bool {
        self.threads_with_tokens > 0 && self.threads_with_tokens < self.threads
    }

    /// Every thread and every model that took turns reported its tokens: shares of the tokens
    /// are shares of all the work, not just of what was reported.
    pub fn tokens_complete(&self) -> bool {
        self.threads_with_tokens == self.threads && self.models.iter().all(|m| m.tokens > 0 || m.turns == 0)
    }

    /// The recap in sentences, with the projects and models as badges. `label` names a model.
    pub fn narrative(&self, label: impl Fn(&AgentId, Option<&str>) -> String) -> Vec<Span> {
        let mut out = vec![];
        if self.is_empty() {
            return out;
        }
        let text = |out: &mut Vec<Span>, s: &str| out.push(Span::Text(s.to_string()));
        let model = |m: &ModelShare| Span::Model { agent: m.agent.clone(), label: label(&m.agent, m.model.as_deref()) };
        text(&mut out, if self.window.range == Range::Week { "This week you sent " } else { "You sent " });
        out.push(Span::Strong(count(self.prompts, "prompt", "prompts")));
        text(&mut out, " across ");
        out.push(Span::Strong(count(self.threads, "thread", "threads")));
        text(&mut out, ".");
        let total_prompts: usize = self.projects.iter().map(|p| p.prompts).sum();
        let mut open = false;
        if let Some(top) = self.projects.first() {
            if self.projects.len() == 1 {
                text(&mut out, " All of it went into ");
            } else if total_prompts > 0 && top.prompts * 2 >= total_prompts {
                text(&mut out, " Most of it went into ");
            } else {
                text(&mut out, &format!(" It was spread over {} projects, led by ", self.projects.len()));
            }
            out.push(Span::Project(top.name.clone()));
            open = true;
        }
        let with_tokens: Vec<&ModelShare> = self.models.iter().filter(|m| m.tokens > 0).collect();
        if let Some(best) = with_tokens.first() {
            if with_tokens.len() == 1 && self.tokens_complete() {
                text(&mut out, if open { ", all on " } else { " It all ran on " });
                out.push(model(best));
            } else if with_tokens.len() == 1 {
                // Others worked too, but only this one said what it used.
                text(&mut out, if open { ", with " } else { " " });
                out.push(model(best));
                text(&mut out, if open { " carrying all of the reported tokens" } else { " carried all of the reported tokens" });
            } else {
                text(&mut out, if open { ", with " } else { " " });
                out.push(model(best));
                text(&mut out, if open { " carrying " } else { " carried " });
                out.push(Span::Strong(format!("{}%", self.token_share(best).unwrap_or(0))));
                text(&mut out, if self.tokens_complete() { " of the tokens, ahead of " } else { " of the reported tokens, ahead of " });
                out.push(model(with_tokens[1]));
            }
            open = true;
        } else if let Some(best) = self.models.iter().find(|m| m.turns > 0) {
            text(&mut out, if open { ", with " } else { " " });
            out.push(model(best));
            if best.turns == self.turns {
                text(&mut out, if self.turns == 1 { " taking the one turn" } else { " taking every turn" });
            } else {
                text(&mut out, " taking ");
                out.push(Span::Strong(format!("{} of {} turns", best.turns, self.turns)));
            }
            open = true;
        }
        if open {
            text(&mut out, ".");
        }
        if self.agent_secs > 0 {
            text(&mut out, " Your agents were on the trail for ");
            out.push(Span::Strong(duration(self.agent_secs)));
            text(&mut out, ".");
        }
        out
    }
}

/// The model `usage` reported the most tokens for in `[from, to]`, if any report named one.
fn busiest_model(usage: &[&UsageRow], from: i64, to: i64) -> Option<String> {
    let mut by: Vec<(String, &str, u64)> = vec![];
    for u in usage.iter().filter(|u| (from..=to).contains(&u.at)) {
        let Some(m) = u.model.as_deref() else { continue };
        let key = model_key(m);
        match by.iter_mut().find(|(k, ..)| *k == key) {
            Some((_, _, n)) => *n += u.tokens.total(),
            None => by.push((key, m, u.tokens.total())),
        }
    }
    by.into_iter().max_by_key(|(.., n)| *n).map(|(_, m, _)| m.to_string())
}

/// Where `model` of `agent` is in `models`, added if it isn't yet.
fn model_at(models: &mut Vec<ModelShare>, agent: &AgentId, model: Option<&str>) -> usize {
    let key = model.map(model_key);
    match models.iter().position(|m| &m.agent == agent && m.model.as_deref().map(model_key) == key) {
        Some(i) => i,
        None => {
            models.push(ModelShare { agent: agent.clone(), model: model.map(String::from), tokens: 0, turns: 0 });
            models.len() - 1
        }
    }
}

/// Add the agent time of a turn running `[from, to)` to the buckets it overlaps.
fn spread(buckets: &mut [Bucket], window: &Window, from: i64, to: i64) {
    let (from, to) = (from.max(window.start), to.min(window.end));
    if to <= from {
        return;
    }
    let (first, last) = (window.bucket(from), window.bucket(to - 1));
    for (i, bucket) in buckets.iter_mut().enumerate().take(last + 1).skip(first) {
        let (b0, b1) = (window.start + i as i64 * window.bucket_ms, window.start + (i as i64 + 1) * window.bucket_ms);
        let overlap = to.min(b1) - from.max(b0);
        if overlap > 0 {
            bucket.agent_secs += (overlap / 1000) as u64;
        }
    }
}

/// A piece of the narrative.
#[derive(Debug, Clone, PartialEq)]
pub enum Span {
    Text(String),
    /// A number worth reading first.
    Strong(String),
    /// A project, shown with its badge.
    Project(String),
    /// A model, shown with its agent's logo.
    Model { agent: AgentId, label: String },
}

/// "1 prompt", "18 prompts".
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Agent time: "1h 2m", "42m", "under a minute".
pub fn duration(secs: u64) -> String {
    match secs {
        0..=59 => "under a minute".into(),
        60..=3_599 => format!("{}m", secs / 60),
        _ if secs % 3_600 < 60 => format!("{}h", secs / 3_600),
        _ => format!("{}h {}m", secs / 3_600, (secs % 3_600) / 60),
    }
}

/// "Good evening" for a local hour.
pub fn greeting(hour: u32) -> &'static str {
    match hour {
        5..=11 => "Good morning",
        12..=16 => "Good afternoon",
        17..=22 => "Good evening",
        _ => "Up late",
    }
}

/// A model's name for people: "Claude Opus 5.5", "GPT-6 Astra". `None` is the agent's default.
pub fn model_label(agent: &AgentId, model: Option<&str>) -> String {
    let Some(model) = model.filter(|m| !m.trim().is_empty()) else { return agent.display_name() };
    // OpenCode and gateways name models `provider/model`.
    let id = model_key(model.rsplit('/').next().unwrap_or(model));
    if !id.starts_with("claude-") && !id.starts_with("gpt-") {
        let known = [agent.clone(), AgentId::ClaudeCode, AgentId::Codex].into_iter().flat_map(|a| catalog::default_models(&a)).find(|m| m.id == id);
        if let Some(m) = known {
            return m.name;
        }
    }
    pretty(&id)
}

/// `claude-opus-5-5` → "Claude Opus 5.5", `gpt-5.6-luna` → "GPT-5.6 Luna": words capitalised,
/// version numbers joined with dots.
fn pretty(id: &str) -> String {
    let mut words: Vec<String> = vec![];
    let mut number = false;
    for part in id.split(['-', '_', ' ']).filter(|p| !p.is_empty()) {
        let numeric = part.chars().all(|c| c.is_ascii_digit() || c == '.');
        match words.last_mut() {
            Some(last) if numeric && number => {
                last.push('.');
                last.push_str(part);
            }
            Some(last) if numeric && last == "GPT" => {
                last.push('-');
                last.push_str(part);
            }
            _ if part == "gpt" => words.push("GPT".into()),
            _ => {
                let mut c = part.chars();
                words.push(c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default());
            }
        }
        number = numeric;
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Item;
    use crate::types::{Effort, HandHolding};
    use chrono::{FixedOffset, NaiveDateTime};

    fn at(tz: &FixedOffset, s: &str) -> i64 {
        let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
        tz.from_local_datetime(&naive).unwrap().timestamp_millis()
    }

    fn now(tz: &FixedOffset, s: &str) -> DateTime<FixedOffset> {
        DateTime::from_timestamp_millis(at(tz, s)).unwrap().with_timezone(tz)
    }

    fn thread(id: &str, agent: AgentId, model: Option<&str>, project: Option<(&str, &str)>) -> ThreadActivity {
        let s = Store::in_memory().unwrap();
        let mut t = s.create_thread(None, agent, model.map(String::from), Effort::High, HandHolding::Auto).unwrap();
        t.id = id.into();
        t.project_id = project.map(|(id, _)| id.to_string());
        ThreadActivity { thread: t, project: project.map(|(_, n)| n.to_string()), activity: vec![], usage: vec![] }
    }

    fn used(t: &ThreadActivity, at: i64, model: Option<&str>, total: u64) -> UsageRow {
        UsageRow { thread_id: t.thread.id.clone(), at, agent: t.thread.agent.clone(), model: model.map(String::from), tokens: TokenUsage { input: total / 10, output: total / 10, cache_read: total - 2 * (total / 10), cache_write: 0 } }
    }

    /// A day across three agents, four models and two projects (plus a thread outside any).
    fn day(tz: &FixedOffset) -> Vec<ThreadActivity> {
        let mut a = thread("a", AgentId::Codex, Some("gpt-6-astra"), Some(("p1", "synara")));
        a.activity = vec![
            Activity::Prompt { at: at(tz, "2026-10-03 09:05") },
            Activity::TurnEnd { at: at(tz, "2026-10-03 09:35"), took_secs: 1_800 },
            Activity::Prompt { at: at(tz, "2026-10-03 14:10") },
            Activity::TurnEnd { at: at(tz, "2026-10-03 14:20"), took_secs: 600 },
            // Yesterday, just before midnight: not today's.
            Activity::Prompt { at: at(tz, "2026-10-02 23:59") },
        ];
        a.usage = vec![used(&a, at(tz, "2026-10-03 09:35"), Some("gpt-6-astra"), 700_000), used(&a, at(tz, "2026-10-03 14:20"), Some("gpt-6-astra"), 70_000)];
        let mut b = thread("b", AgentId::ClaudeCode, Some("claude-opus-5-5"), Some(("p1", "synara")));
        b.activity = vec![Activity::Prompt { at: at(tz, "2026-10-03 14:00") }, Activity::TurnEnd { at: at(tz, "2026-10-03 14:02"), took_secs: 120 }];
        // A sub-agent on Haiku, reported under its dated id.
        b.usage = vec![used(&b, at(tz, "2026-10-03 14:02"), Some("claude-opus-5-5"), 200_000), used(&b, at(tz, "2026-10-03 14:02"), Some("claude-haiku-4-5-20251001"), 30_000)];
        let mut c = thread("c", AgentId::OpenCode, None, Some(("p2", "trek")));
        c.activity = vec![Activity::Prompt { at: at(tz, "2026-10-03 20:00") }, Activity::TurnEnd { at: at(tz, "2026-10-03 20:01"), took_secs: 60 }];
        c.thread.run_state = RunState::Failed;
        c.thread.updated_at = at(tz, "2026-10-03 20:01");
        let mut d = thread("d", AgentId::ClaudeCode, Some("claude-opus-5-5"), None);
        d.activity = vec![Activity::Prompt { at: at(tz, "2026-10-03 21:00") }];
        // Nothing in the window at all.
        let mut e = thread("e", AgentId::Codex, None, Some(("p3", "old")));
        e.activity = vec![Activity::Prompt { at: at(tz, "2026-10-01 10:00") }];
        vec![a, b, c, d, e]
    }

    #[test]
    fn a_share_rounds_to_all_or_none_only_when_it_is() {
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let window = Range::Today.window(&now(&tz, "2026-10-03 22:30"));
        let mut r = Recap::compute(window, at(&tz, "2026-10-03 22:30"), &day(&tz));
        let share = |agent: AgentId, tokens| ModelShare { agent, model: None, tokens, turns: 1 };
        r.models = vec![share(AgentId::ClaudeCode, 111_000_000), share(AgentId::Codex, 4_000)];
        assert_eq!(r.token_share(&r.models[0]), Some(99), "another model used some");
        assert_eq!(r.token_share(&r.models[1]), Some(1));
        let said = r.narrative(|a, _| a.display_name()).iter().map(|s| format!("{s:?}")).collect::<String>();
        assert!(said.contains("99%") && !said.contains("100%"), "{said}");
        r.models.truncate(1);
        assert_eq!(r.token_share(&r.models[0]), Some(100));
    }

    #[test]
    fn a_day_adds_up_across_agents_models_and_projects() {
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let window = Range::Today.window(&now(&tz, "2026-10-03 22:30"));
        let r = Recap::compute(window, at(&tz, "2026-10-03 22:30"), &day(&tz));
        assert_eq!((r.prompts, r.threads, r.turns), (5, 4, 4));
        assert_eq!(r.agent_secs, 1_800 + 600 + 120 + 60);
        assert_eq!(r.tokens.total(), 1_000_000);
        assert_eq!((r.threads_with_tokens, r.failed), (2, 1));
        assert!(r.tokens_partial());
        assert_eq!(r.projects.iter().map(|p| (p.name.as_str(), p.prompts)).collect::<Vec<_>>(), [("synara", 3), ("trek", 1)]);
        assert_eq!(r.projects[0].tokens, 1_000_000);
        let models: Vec<(Option<&str>, u64, usize)> = r.models.iter().map(|m| (m.model.as_deref(), m.tokens, m.turns)).collect();
        assert_eq!(models, [(Some("gpt-6-astra"), 770_000, 2), (Some("claude-opus-5-5"), 200_000, 1), (Some("claude-haiku-4-5-20251001"), 30_000, 0), (None, 0, 1)]);
        assert_eq!(r.token_share(&r.models[0]), Some(77));
        assert_eq!(r.token_share(&r.models[3]), None, "reported no tokens");
        // 24 hourly buckets; the 09:05 turn ran 09:05–09:35, the peak.
        assert_eq!(r.buckets.len(), 24);
        assert_eq!(r.buckets[9].agent_secs, 1_800);
        assert_eq!(r.buckets[14].prompts, 2);
        assert_eq!(r.peak(), Some(9));
        assert_eq!(r.now_bucket(), Some(22));
        assert_eq!(r.buckets.iter().map(|b| b.tokens).sum::<u64>(), 1_000_000);
    }

    #[test]
    fn turns_spread_over_the_hours_they_ran() {
        let tz = FixedOffset::east_opt(0).unwrap();
        let mut t = thread("a", AgentId::Codex, None, None);
        // 10:30 to 12:15.
        t.activity = vec![Activity::TurnEnd { at: at(&tz, "2026-10-03 12:15"), took_secs: 105 * 60 }];
        let r = Recap::compute(Range::Today.window(&now(&tz, "2026-10-03 13:00")), at(&tz, "2026-10-03 13:00"), &[t]);
        assert_eq!(r.buckets[10].agent_secs, 30 * 60);
        assert_eq!(r.buckets[11].agent_secs, 60 * 60);
        assert_eq!(r.buckets[12].agent_secs, 15 * 60);
        // A turn that began before midnight counts from midnight.
        let mut t = thread("b", AgentId::Codex, None, None);
        t.activity = vec![Activity::TurnEnd { at: at(&tz, "2026-10-03 00:10"), took_secs: 20 * 60 }];
        let r = Recap::compute(Range::Today.window(&now(&tz, "2026-10-03 13:00")), 0, &[t]);
        assert_eq!(r.buckets[0].agent_secs, 10 * 60);
    }

    #[test]
    fn days_and_weeks_follow_the_local_calendar() {
        // 00:30 on Saturday in UTC+9 is still Friday in UTC: the day is the local one.
        let tokyo = FixedOffset::east_opt(9 * 3600).unwrap();
        let w = Range::Today.window(&now(&tokyo, "2026-10-03 00:30"));
        assert_eq!(w.start, at(&tokyo, "2026-10-03 00:00"));
        assert_eq!(w.end - w.start, 24 * 3_600_000);
        let ny = FixedOffset::west_opt(4 * 3600).unwrap();
        let w = Range::Today.window(&now(&ny, "2026-10-03 23:59"));
        assert_eq!(w.start, at(&ny, "2026-10-03 00:00"));
        assert!(w.contains(at(&ny, "2026-10-03 23:59")) && !w.contains(at(&ny, "2026-10-04 00:00")));
        // The week runs from Monday: Saturday 3 October 2026 is in the week of the 28th.
        let w = Range::Week.window(&now(&ny, "2026-10-03 12:00"));
        assert_eq!(w.start, at(&ny, "2026-09-28 00:00"));
        assert_eq!(w.end, at(&ny, "2026-10-05 00:00"));
        assert_eq!(w.buckets(), 56);
        // On a Monday the week is just that day so far.
        let w = Range::Week.window(&now(&ny, "2026-10-05 08:00"));
        assert_eq!(w.start, at(&ny, "2026-10-05 00:00"));
    }

    #[test]
    fn a_week_counts_what_the_day_leaves_out() {
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let threads = day(&tz);
        let week = Recap::compute(Range::Week.window(&now(&tz, "2026-10-03 22:30")), at(&tz, "2026-10-03 22:30"), &threads);
        // Yesterday's prompt and Thursday's thread join in.
        assert_eq!((week.prompts, week.threads), (7, 5));
        assert_eq!(week.projects.len(), 3);
        assert_eq!(week.buckets[0].start, at(&tz, "2026-09-28 00:00"));
        // Saturday 09:05 is bucket 5 days × 8 + 3.
        assert_eq!(week.buckets[43].prompts, 1);
        // Next week's recap holds none of it.
        let next = Recap::compute(Range::Week.window(&now(&tz, "2026-10-06 10:00")), at(&tz, "2026-10-06 10:00"), &threads);
        assert!(next.is_empty() && next.narrative(model_label).is_empty() && next.peak().is_none());
    }

    fn words(spans: &[Span]) -> String {
        spans
            .iter()
            .map(|s| match s {
                Span::Text(t) => t.clone(),
                Span::Strong(t) => format!("*{t}*"),
                Span::Project(p) => format!("[{p}]"),
                Span::Model { label, .. } => format!("<{label}>"),
            })
            .collect()
    }

    #[test]
    fn the_narrative_says_what_is_known() {
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let r = Recap::compute(Range::Today.window(&now(&tz, "2026-10-03 22:30")), 0, &day(&tz));
        assert_eq!(
            words(&r.narrative(model_label)),
            "You sent *5 prompts* across *4 threads*. Most of it went into [synara], with <GPT-6 Astra> carrying *77%* of the reported tokens, ahead of <Claude Opus 5.5>. Your agents were on the trail for *43m*."
        );
        // No tokens reported: turns tell instead, and nothing is said about tokens.
        let mut quiet = day(&tz);
        for t in &mut quiet {
            t.usage.clear();
        }
        let r = Recap::compute(Range::Today.window(&now(&tz, "2026-10-03 22:30")), 0, &quiet);
        let said = words(&r.narrative(model_label));
        assert!(said.contains("with <GPT-6 Astra> taking *2 of 4 turns*."), "{said}");
        assert!(!said.contains("token"), "{said}");
        // One thread, one model, one project, the week.
        let mut one = thread("x", AgentId::ClaudeCode, Some("claude-opus-5-5"), Some(("p", "trek")));
        let t0 = at(&tz, "2026-10-03 10:00");
        one.activity = vec![Activity::Prompt { at: t0 }, Activity::TurnEnd { at: t0 + 30_000, took_secs: 30 }];
        one.usage = vec![used(&one, t0 + 30_000, Some("claude-opus-5-5"), 1_000)];
        let r = Recap::compute(Range::Week.window(&now(&tz, "2026-10-03 22:30")), 0, &[one]);
        assert_eq!(
            words(&r.narrative(model_label)),
            "This week you sent *1 prompt* across *1 thread*. All of it went into [trek], all on <Claude Opus 5.5>. Your agents were on the trail for *under a minute*."
        );
    }

    #[test]
    fn one_model_with_tokens_among_others_isnt_called_the_only_one() {
        let tz = FixedOffset::east_opt(0).unwrap();
        let t0 = at(&tz, "2026-10-03 10:00");
        let mut claude = thread("a", AgentId::ClaudeCode, Some("claude-opus-5-5"), Some(("p", "trek")));
        claude.activity = vec![Activity::Prompt { at: t0 }, Activity::TurnEnd { at: t0 + 60_000, took_secs: 60 }];
        claude.usage = vec![used(&claude, t0 + 60_000, Some("claude-opus-5-5"), 5_000)];
        // A Codex thread that took turns but said nothing of its tokens.
        let mut codex = thread("b", AgentId::Codex, Some("gpt-5.6-luna"), Some(("p", "trek")));
        codex.activity = vec![Activity::Prompt { at: t0 + 120_000 }, Activity::TurnEnd { at: t0 + 180_000, took_secs: 60 }];
        let window = Range::Today.window(&now(&tz, "2026-10-03 12:00"));
        let r = Recap::compute(window, 0, &[claude.clone(), codex]);
        assert!(!r.tokens_complete());
        let said = words(&r.narrative(model_label));
        assert!(said.contains("All of it went into [trek], with <Claude Opus 5.5> carrying all of the reported tokens."), "{said}");
        assert!(!said.contains("all on"), "{said}");
        // On its own it is the only one.
        let r = Recap::compute(window, 0, &[claude]);
        assert!(r.tokens_complete());
        assert!(words(&r.narrative(model_label)).contains(", all on <Claude Opus 5.5>."));
    }

    #[test]
    fn turns_go_to_the_model_that_ran_them() {
        let tz = FixedOffset::east_opt(0).unwrap();
        let t0 = at(&tz, "2026-10-03 10:00");
        // Left on the agent's default: the turns go where its reports say.
        let mut a = thread("a", AgentId::ClaudeCode, None, None);
        a.activity = vec![Activity::TurnEnd { at: t0, took_secs: 60 }, Activity::TurnEnd { at: t0 + 3_600_000, took_secs: 60 }];
        a.usage = vec![used(&a, t0 + 5, Some("claude-opus-5-5"), 9_000), used(&a, t0 + 5, Some("claude-haiku-4-5-20251001"), 100)];
        // Switched model halfway: each turn on the model it ran on.
        let mut b = thread("b", AgentId::Codex, Some("gpt-6-astra"), None);
        b.activity = vec![Activity::TurnEnd { at: t0, took_secs: 60 }, Activity::TurnEnd { at: t0 + 3_600_000, took_secs: 60 }];
        b.usage = vec![used(&b, t0, Some("gpt-5.6-luna"), 1_000), used(&b, t0 + 3_600_000, Some("gpt-6-astra"), 1_000)];
        let r = Recap::compute(Range::Today.window(&now(&tz, "2026-10-03 12:00")), 0, &[a, b]);
        let turns = |m: &str| r.models.iter().find(|x| x.model.as_deref() == Some(m)).map(|x| x.turns);
        assert_eq!(turns("claude-opus-5-5"), Some(2));
        assert_eq!(turns("claude-haiku-4-5-20251001"), Some(0));
        assert_eq!((turns("gpt-5.6-luna"), turns("gpt-6-astra")), (Some(1), Some(1)));
        assert!(r.models.iter().all(|m| m.model.is_some()), "{:?}", r.models);
    }

    #[test]
    fn failed_and_stopped_turns_count_as_work_and_failures_are_counted_per_turn() {
        let tz = FixedOffset::east_opt(0).unwrap();
        let t0 = at(&tz, "2026-10-03 10:00");
        let mut a = thread("a", AgentId::Codex, None, None);
        a.activity = vec![
            Activity::Prompt { at: t0 },
            Activity::TurnStopped { at: t0 + 1_200_000, took_secs: 1_200, failed: true },
            Activity::Prompt { at: t0 + 2_000_000 },
            Activity::TurnStopped { at: t0 + 2_100_000, took_secs: 100, failed: false },
            Activity::TurnStopped { at: t0 + 3_000_000, took_secs: 10, failed: true },
        ];
        // Failed now, but the failure was recorded: not counted twice.
        a.thread.run_state = RunState::Failed;
        a.thread.updated_at = t0 + 3_000_000;
        // Failed yesterday: not today's failure.
        let mut b = thread("b", AgentId::Codex, None, None);
        b.thread.run_state = RunState::Failed;
        b.thread.updated_at = at(&tz, "2026-10-02 18:00");
        b.activity = vec![Activity::Prompt { at: at(&tz, "2026-10-02 17:00") }];
        let r = Recap::compute(Range::Today.window(&now(&tz, "2026-10-03 12:00")), 0, &[a, b]);
        assert_eq!((r.turns, r.agent_secs, r.failed, r.threads), (3, 1_310, 2, 1));
        assert_eq!(r.buckets[10].agent_secs, 1_310);
    }

    /// A time zone with one clock change at `switch` (UTC), like a real zone's spring or fall.
    #[derive(Debug, Clone, Copy)]
    struct Shifting {
        switch: i64,
        before: i32,
        after: i32,
    }

    #[derive(Debug, Clone, Copy)]
    struct ShiftingOffset(Shifting, FixedOffset);

    impl chrono::Offset for ShiftingOffset {
        fn fix(&self) -> FixedOffset {
            self.1
        }
    }

    impl TimeZone for Shifting {
        type Offset = ShiftingOffset;
        fn from_offset(offset: &ShiftingOffset) -> Self {
            offset.0
        }
        fn offset_from_local_date(&self, local: &NaiveDate) -> chrono::MappedLocalTime<ShiftingOffset> {
            self.offset_from_local_datetime(&local.and_hms_opt(0, 0, 0).unwrap())
        }
        fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> chrono::MappedLocalTime<ShiftingOffset> {
            let local = local.and_utc().timestamp_millis();
            let mut fits: Vec<(i64, ShiftingOffset)> = [self.before, self.after]
                .into_iter()
                .map(|secs| (local - secs as i64 * 1000, secs))
                .filter(|(utc, secs)| (*utc < self.switch) == (*secs == self.before))
                .map(|(utc, secs)| (utc, ShiftingOffset(*self, FixedOffset::east_opt(secs).unwrap())))
                .collect();
            fits.sort_by_key(|(utc, _)| *utc);
            match &fits[..] {
                [] => chrono::MappedLocalTime::None,
                [(_, o)] => chrono::MappedLocalTime::Single(*o),
                [(_, a), (_, b), ..] => chrono::MappedLocalTime::Ambiguous(*a, *b),
            }
        }
        fn offset_from_utc_date(&self, utc: &NaiveDate) -> ShiftingOffset {
            self.offset_from_utc_datetime(&utc.and_hms_opt(0, 0, 0).unwrap())
        }
        fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> ShiftingOffset {
            let secs = if utc.and_utc().timestamp_millis() < self.switch { self.before } else { self.after };
            ShiftingOffset(*self, FixedOffset::east_opt(secs).unwrap())
        }
    }

    fn utc(s: &str) -> i64 {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap().and_utc().timestamp_millis()
    }

    #[test]
    fn days_with_a_clock_change_are_as_long_as_they_are() {
        const H: i64 = 3_600_000;
        // Spring: 02:00 jumps to 03:00 (UTC+1 to UTC+2), a 23-hour day.
        let spring = Shifting { switch: utc("2026-03-29 01:00"), before: 3600, after: 7200 };
        let w = Range::Today.window(&spring.timestamp_millis_opt(utc("2026-03-29 10:00")).unwrap());
        assert_eq!((w.start, w.end), (utc("2026-03-28 23:00"), utc("2026-03-29 22:00")));
        assert_eq!(w.buckets(), 23);
        // Autumn: 03:00 goes back to 02:00, a 25-hour day.
        let fall = Shifting { switch: utc("2026-10-25 01:00"), before: 7200, after: 3600 };
        let w = Range::Today.window(&fall.timestamp_millis_opt(utc("2026-10-25 10:00")).unwrap());
        assert_eq!(w.end - w.start, 25 * H);
        assert_eq!(w.buckets(), 25);
        // Where the change skips midnight itself (00:00 → 01:00), the day starts at 01:00.
        let skip = Shifting { switch: utc("2026-09-06 04:00"), before: -4 * 3600, after: -3 * 3600 };
        let w = Range::Today.window(&skip.timestamp_millis_opt(utc("2026-09-06 15:00")).unwrap());
        assert_eq!(w.start, utc("2026-09-06 04:00"));
        assert_eq!(w.end - w.start, 23 * H);
        // A week across the change: 7 days less the hour.
        let w = Range::Week.window(&spring.timestamp_millis_opt(utc("2026-03-29 10:00")).unwrap());
        assert_eq!(w.start, utc("2026-03-22 23:00"));
        assert_eq!(w.end - w.start, 7 * 24 * H - H);
    }

    #[test]
    fn models_are_named_for_people() {
        assert_eq!(model_label(&AgentId::ClaudeCode, Some("claude-opus-5-5")), "Claude Opus 5.5");
        assert_eq!(model_label(&AgentId::ClaudeCode, Some("claude-haiku-4-5-20251001")), "Claude Haiku 4.5");
        assert_eq!(model_label(&AgentId::Codex, Some("gpt-6-astra")), "GPT-6 Astra");
        assert_eq!(model_label(&AgentId::Codex, Some("gpt-5.6-luna")), "GPT-5.6 Luna");
        assert_eq!(model_label(&AgentId::OpenCode, Some("anthropic/claude-sonnet-5-5")), "Claude Sonnet 5.5");
        assert_eq!(model_label(&AgentId::Direct("mock".into()), Some("mock-swift")), "Mock Swift");
        assert_eq!(model_label(&AgentId::Codex, None), "Codex");
        assert_eq!(model_label(&AgentId::OpenCode, Some("big-pickle")), "Big Pickle");
    }

    #[test]
    fn times_and_greetings_read_naturally() {
        assert_eq!(duration(3_720), "1h 2m");
        assert_eq!(duration(7_200), "2h");
        assert_eq!(duration(2_520), "42m");
        assert_eq!(greeting(21), "Good evening");
        assert_eq!(greeting(8), "Good morning");
        assert_eq!(greeting(2), "Up late");
    }

    #[test]
    fn gather_reads_transcripts_usage_and_projects_from_the_store() {
        let s = Store::in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("trek-basecamp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = s.create_thread(Some(&dir), AgentId::ClaudeCode, Some("claude-opus-5-5".into()), Effort::High, HandHolding::Auto).unwrap();
        let tz = FixedOffset::east_opt(0).unwrap();
        let window = Range::Today.window(&now(&tz, "2099-10-03 12:00"));
        // Created now, the thread has been active since long before the window: move it into it.
        s.update_thread(&t.id, |t| t.updated_at = window.start + 1).unwrap();
        let p = window.start + 3_600_000;
        let items = vec![
            Item::User { text: "go".into(), images: vec![], at: Some(p), resume: None, aside: false },
            Item::Assistant { text: "done".into() },
            Item::TurnEnd { at: p + 60_000, took_secs: 60 },
        ];
        s.save_transcript(&t.id, &mut crate::transcript::Transcript::unsaved(items)).unwrap();
        s.record_usage(&t.id, p + 60_000, &AgentId::ClaudeCode, Some("claude-opus-5-5"), &TokenUsage { input: 5, output: 50, cache_read: 1_000, cache_write: 0 }).unwrap();
        let got = gather(&s, &window).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].project.as_deref(), dir.file_name().and_then(|n| n.to_str()));
        assert_eq!(got[0].activity, vec![Activity::Prompt { at: p }, Activity::TurnEnd { at: p + 60_000, took_secs: 60 }]);
        let r = Recap::compute(window, p + 120_000, &got);
        assert_eq!((r.prompts, r.turns, r.tokens.total()), (1, 1, 1_055));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sub_agents_work_under_their_models_but_are_no_threads_or_prompts_of_the_users() {
        let s = Store::in_memory().unwrap();
        let tz = FixedOffset::east_opt(0).unwrap();
        let window = Range::Today.window(&now(&tz, "2099-10-03 12:00"));
        let p = window.start + 3_600_000;
        let user = |text: &str, at: i64| Item::User { text: text.into(), images: vec![], at: Some(at), resume: None, aside: false };
        let parent = s.create_thread(None, AgentId::ClaudeCode, Some("claude-opus-5-5".into()), Effort::High, HandHolding::Auto).unwrap();
        let mut child = s.create_thread(None, AgentId::Codex, Some("gpt-6.1-sol".into()), Effort::High, HandHolding::Supervised).unwrap();
        child.parent_id = Some(parent.id.clone());
        s.save_thread(&child).unwrap();
        for id in [&parent.id, &child.id] {
            s.update_thread(id, |t| t.updated_at = window.start + 1).unwrap();
        }
        let wake = crate::orchestrate::wake_text(&[crate::orchestrate::Report {
            id: child.id.clone(),
            title: "Review".into(),
            model: "Sol".into(),
            outcome: crate::orchestrate::Outcome::Done("Looks right.".into()),
        }]);
        let mut parent_items = crate::transcript::Transcript::unsaved(vec![
            user("Ask Sol", p),
            Item::Assistant { text: "Asked.".into() },
            Item::TurnEnd { at: p + 10_000, took_secs: 10 },
            user(&wake, p + 70_000),
            Item::Assistant { text: "Sol agrees.".into() },
            Item::TurnEnd { at: p + 80_000, took_secs: 10 },
        ]);
        s.save_transcript(&parent.id, &mut parent_items).unwrap();
        let mut child_items =
            crate::transcript::Transcript::unsaved(vec![user("Review the cache", p + 5_000), Item::Assistant { text: "Looks right.".into() }, Item::TurnEnd { at: p + 65_000, took_secs: 60 }]);
        s.save_transcript(&child.id, &mut child_items).unwrap();
        s.record_usage(&child.id, p + 65_000, &AgentId::Codex, Some("gpt-6.1-sol"), &TokenUsage { input: 100, output: 900, cache_read: 0, cache_write: 0 }).unwrap();
        let got = gather(&s, &window).unwrap();
        let r = Recap::compute(window, p + 120_000, &got);
        assert_eq!((r.prompts, r.threads, r.turns), (1, 1, 3), "the user asked once, in one thread; three turns were worked");
        assert_eq!(r.agent_secs, 80);
        let sol = r.models.iter().find(|m| m.model.as_deref() == Some("gpt-6.1-sol")).expect("the sub-agent's model");
        assert_eq!((sol.tokens, sol.turns), (1_000, 1));
    }
}
