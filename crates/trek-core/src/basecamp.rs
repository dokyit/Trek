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
/// By (source, session, window start): the thread's `updated_at` when it was read, and what it held.
type ImportedKey = (String, String, i64);
static IMPORTED: LazyLock<Mutex<HashMap<ImportedKey, (i64, Imported)>>> = LazyLock::new(Default::default);

/// Everything a recap of `window` needs, read from the store and (for imported threads not
/// continued here) the agents' files. Slow: run it off the main thread.
pub fn gather(store: &Store, window: &Window) -> anyhow::Result<Vec<ThreadActivity>> {
    let threads = store.threads_since(window.start)?;
    let names: HashMap<String, String> = store.projects()?.into_iter().map(|p| (p.id, p.name)).collect();
    let ids: Vec<String> = threads.iter().map(|t| t.id.clone()).collect();
    let stored = store.with_transcripts(&ids)?;
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
                    let found = imported(&t, source, native, window);
                    (found.0.clone(), found.1.clone())
                }
                _ => (activity.remove(&t.id).unwrap_or_default(), usage.remove(&t.id).unwrap_or_default()),
            };
            let project = t.project_id.as_ref().and_then(|p| names.get(p).cloned());
            ThreadActivity { thread: t, project, activity, usage }
        })
        .collect())
}

/// An imported thread's prompts, turns and tokens in `window`, from its agent's history. Read
/// again only once the thread has moved on since.
fn imported(t: &Thread, source: ThreadSource, native: &str, window: &Window) -> Imported {
    let key = (source.key().to_string(), native.to_string(), window.start);
    if let Some((at, found)) = IMPORTED.lock().expect("basecamp cache").get(&key)
        && *at == t.updated_at
    {
        return found.clone();
    }
    let items = import::load_transcript(source, native).unwrap_or_else(|e| {
        tracing::debug!("basecamp: {} {native}: {e:#}", source.key());
        vec![]
    });
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
    let usage = import::load_usage(source, native, window.start, window.end)
        .into_iter()
        .map(|(at, model, tokens)| UsageRow { thread_id: t.id.clone(), at, agent: agent.clone(), model: model.or_else(|| t.model.clone()), tokens })
        .collect();
    let found = Arc::new((activity, usage));
    IMPORTED.lock().expect("basecamp cache").insert(key, (t.updated_at, found.clone()));
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

/// A model's part in a recap: the tokens it reported and the turns of threads that use it.
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
    /// Threads whose last turn in the window failed.
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
            for a in t.activity.iter().filter(|a| window.contains(a.at())) {
                match *a {
                    Activity::Prompt { at } => {
                        prompts += 1;
                        buckets[window.bucket(at)].prompts += 1;
                    }
                    Activity::TurnEnd { at, took_secs } => {
                        turns += 1;
                        recap.agent_secs += took_secs as u64;
                        spread(&mut buckets, &window, at - took_secs as i64 * 1000, at);
                    }
                }
            }
            let usage: Vec<&UsageRow> = t.usage.iter().filter(|u| window.contains(u.at)).collect();
            if prompts == 0 && turns == 0 && usage.is_empty() {
                continue;
            }
            recap.threads += 1;
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
            if tokens > 0 {
                recap.threads_with_tokens += 1;
            }
            if turns > 0 {
                let i = model_at(&mut models, &t.thread.agent, t.thread.model.as_deref());
                models[i].turns += turns;
            }
            if t.thread.run_state == RunState::Failed && window.contains(t.thread.updated_at) {
                recap.failed += 1;
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

    /// Share of the reported tokens `m` used, in percent.
    pub fn token_share(&self, m: &ModelShare) -> Option<u32> {
        let total: u64 = self.models.iter().map(|m| m.tokens).sum();
        (total > 0 && m.tokens > 0).then(|| ((m.tokens as f64 / total as f64) * 100.).round() as u32)
    }

    /// Tokens weren't reported for every thread that worked.
    pub fn tokens_partial(&self) -> bool {
        self.threads_with_tokens > 0 && self.threads_with_tokens < self.threads
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
            if with_tokens.len() == 1 {
                text(&mut out, if open { ", all on " } else { " It all ran on " });
                out.push(model(best));
            } else {
                text(&mut out, if open { ", with " } else { " " });
                out.push(model(best));
                text(&mut out, " carrying ");
                out.push(Span::Strong(format!("{}%", self.token_share(best).unwrap_or(0))));
                text(&mut out, if self.tokens_partial() { " of the reported tokens, ahead of " } else { " of the tokens, ahead of " });
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
}
