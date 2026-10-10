//! The API cost estimate under the composer: what a thread's tokens cost at API prices (its
//! sub-agents' included), the label the status strip shows, and the breakdown behind it.

use trek_agents::Billing;
use trek_core::pricing::{ModelSpend, Provider, Spend};

/// What a thread spent, and what the sub-agents it started (and theirs) spent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThreadSpend {
    pub own: Spend,
    pub subs: Spend,
    /// Sub-agents that used any tokens.
    pub sub_threads: usize,
    /// They were all consulted (advise mode), as far as Trek knows.
    pub consults: bool,
}

impl ThreadSpend {
    fn usd(&self) -> f64 {
        self.own.usd() + self.subs.usd()
    }

    fn priced(&self) -> bool {
        self.own.priced() || self.subs.priced()
    }

    fn tokens(&self) -> u64 {
        self.own.tokens() + self.subs.tokens()
    }

    fn unpriced(&self) -> u64 {
        self.own.unpriced() + self.subs.unpriced()
    }
}

/// A token count, to a tenth of a thousand below 100K: "850", "12.3K", "182K", "2.1M".
pub fn fmt_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=99_999 => format!("{:.1}K", n as f64 / 1_000.).replace(".0K", "K"),
        _ => crate::workspace::fmt_tokens(n),
    }
}

/// Dollars as the strip shows them: cents, or "< $0.01" for less than a cent.
pub fn usd(x: f64) -> String {
    if x > 0.0 && x < 0.005 {
        return "< $0.01".into();
    }
    let cents = (x * 100.0).round() as i64;
    let dollars = cents / 100;
    let mut grouped = String::new();
    for (i, c) in dollars.to_string().chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            grouped.insert(0, ',');
        }
        grouped.insert(0, c);
    }
    format!("${grouped}.{:02}", cents % 100)
}

/// A price per million tokens: "$2", "$0.20", "$0.075".
fn rate(x: f64) -> String {
    if x == 0.0 {
        return "free".into();
    }
    let s = if x >= 1.0 && x.fract() == 0.0 { format!("{x:.0}") } else if (x * 100.0).fract().abs() < 1e-9 { format!("{x:.2}") } else { format!("{x}") };
    format!("${s}/M")
}

/// "your Claude Max plan", or "your subscription" when the plan has no name.
pub fn plan_phrase(plan: &Option<String>) -> String {
    plan.as_deref().map(|p| format!("your {p} plan")).unwrap_or_else(|| "your subscription".into())
}

/// How a session is billed, in words: the tooltip over the agent's name under the composer.
pub fn billing_note(billing: Option<&Billing>) -> Option<String> {
    match billing? {
        Billing::Plan(plan) => Some(format!("Included in {}", plan_phrase(plan))),
        Billing::Metered => Some("Billed per token by your API provider".to_string()),
        Billing::Local => Some(format!("Runs on {}, nothing is billed", crate::words::words().this_computer)),
    }
}

/// The status strip's text: "≈ $1.24 at API prices" (a plan covers it, or how it's billed isn't
/// known), "$1.24" (billed per token; "$1.00 + ≈ $0.30 from consults" when sub-agents, which
/// may run on a plan, spent some of it), "12.3K tokens · price unknown" (nothing has a price),
/// "202K tokens · free" (the models are free), or nothing (no tokens yet, or a model on this
/// Mac).
pub fn label(billing: Option<&Billing>, s: &ThreadSpend) -> Option<String> {
    if matches!(billing, Some(Billing::Local)) || s.tokens() == 0 && s.usd() == 0.0 {
        return None;
    }
    if !s.priced() {
        return Some(format!("{} tokens · price unknown", fmt_tokens(s.unpriced())));
    }
    if s.usd() == 0.0 {
        return Some(format!("{} tokens · free", fmt_tokens(s.tokens())));
    }
    Some(match billing {
        // Sub-agents may run on a plan: only the thread's own spend is what was billed.
        Some(Billing::Metered) if s.subs.usd() > 0.0 => format!("{} + ≈ {} from {}", usd(s.own.usd()), usd(s.subs.usd()), if s.consults { "consults" } else { "sub-agents" }),
        Some(Billing::Metered) => usd(s.usd()),
        _ => format!("≈ {} at API prices", usd(s.usd())),
    })
}

/// How much of a thread's prompts the agents' prompt caches served (sub-agents included).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheHits {
    /// Prompt tokens read from a cache, written to one, and sent fresh.
    pub read: u64,
    pub written: u64,
    pub fresh: u64,
}

impl CacheHits {
    pub fn of(s: &ThreadSpend) -> Option<Self> {
        let mut c = CacheHits { read: 0, written: 0, fresh: 0 };
        for m in s.own.models.iter().chain(&s.subs.models) {
            c.read += m.tokens.cache_read;
            c.written += m.tokens.cache_write;
            c.fresh += m.tokens.input;
        }
        (c.prompt() > 0).then_some(c)
    }

    fn prompt(&self) -> u64 {
        self.read + self.written + self.fresh
    }

    /// The share of prompt tokens read from a cache, 0 to 1.
    pub fn ratio(&self) -> f64 {
        self.read as f64 / self.prompt().max(1) as f64
    }

    /// The status strip's text: "82% cached".
    pub fn label(&self) -> String {
        let pct = self.ratio() * 100.0;
        // 99.6% isn't "100%": all of it is only all of it.
        let shown = if self.read > 0 && pct < 1.0 { "< 1".to_string() } else if self.read < self.prompt() && pct > 99.0 { format!("{:.0}", pct.floor()) } else { format!("{pct:.0}") };
        format!("{shown}% cached")
    }

    /// Its tooltip, in words.
    pub fn detail(&self) -> String {
        let mut s = format!("{} of {} prompt tokens came from the prompt cache, cheaper and faster than sending them again.", fmt_tokens(self.read), fmt_tokens(self.prompt()));
        if self.written > 0 {
            s.push_str(&format!(" {} were written to it for later turns.", fmt_tokens(self.written)));
        }
        s
    }
}

/// One kind of token in a model's line: what it is, how many, at what price.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub kind: &'static str,
    pub tokens: String,
    pub rate: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelLine {
    pub name: String,
    /// What it cost, or "price unknown".
    pub amount: String,
    pub rows: Vec<Row>,
}

/// What the tooltip over the label says.
#[derive(Debug, Clone, PartialEq)]
pub struct Breakdown {
    pub headline: String,
    /// How it's billed: "Included in your Claude Max plan", "Billed per token by your API provider".
    pub billing: Option<String>,
    pub models: Vec<ModelLine>,
    /// "+ $0.30 from 2 consults".
    pub subs: Option<String>,
    /// Tokens left out of the total, for want of a price.
    pub unpriced: Option<String>,
    /// Where the figures come from: the agent's own pricing, the price tables and their dates.
    pub sources: Vec<String>,
}

fn model_line(m: &ModelSpend) -> ModelLine {
    let name = match (m.price, &m.model) {
        (Some(p), _) => p.name.to_string(),
        (None, Some(id)) => id.clone(),
        (None, None) => "Unnamed model".into(),
    };
    let rates = m.price.map(|p| p.rates);
    let mut rows = Vec::new();
    let mut row = |kind: &'static str, n: u64, r: Option<String>| {
        if n > 0 {
            rows.push(Row { kind, tokens: fmt_tokens(n), rate: r });
        }
    };
    row("Input", m.tokens.input, rates.map(|r| rate(r.input)));
    row(
        "Cache write",
        m.tokens.cache_write,
        rates.map(|r| match r.cache_write_1h {
            Some(long) => format!("{} (1h: {})", rate(r.cache_write), rate(long)),
            None => rate(r.cache_write),
        }),
    );
    row("Cache read", m.tokens.cache_read, rates.map(|r| rate(r.cache_read)));
    row("Output", m.tokens.output, rates.map(|r| rate(r.output)));
    let amount = if m.unpriced >= m.tokens.total() && m.usd == 0.0 { "price unknown".into() } else { usd(m.usd) };
    ModelLine { name, amount, rows }
}

/// "Oct 3, 2026" from "2026-10-03".
fn day(d: &str) -> String {
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").map(|d| d.format("%b %-d, %Y").to_string()).unwrap_or_else(|_| d.into())
}

/// The tooltip's breakdown, `None` when the label is.
pub fn breakdown(billing: Option<&Billing>, s: &ThreadSpend) -> Option<Breakdown> {
    label(billing, s)?;
    let headline = if !s.priced() {
        format!("{} tokens · price unknown", fmt_tokens(s.unpriced()))
    } else if s.usd() == 0.0 {
        format!("{} tokens · free", fmt_tokens(s.tokens()))
    } else if matches!(billing, Some(Billing::Metered)) {
        format!("{} so far", usd(s.own.usd()))
    } else {
        format!("≈ {} at API prices", usd(s.usd()))
    };
    let billing_line = match billing {
        Some(Billing::Plan(plan)) => Some(format!("Included in {}", plan_phrase(plan))),
        Some(Billing::Metered) => Some("Billed per token by your API provider".into()),
        _ => None,
    };
    let models = s.own.sorted().into_iter().map(model_line).collect();
    let subs = (s.sub_threads > 0 && !s.subs.is_empty()).then(|| {
        let who = match (s.consults, s.sub_threads) {
            (true, 1) => "1 consult".to_string(),
            (true, n) => format!("{n} consults"),
            (false, 1) => "1 sub-agent".to_string(),
            (false, n) => format!("{n} sub-agents"),
        };
        if !s.subs.priced() {
            format!("+ {} tokens from {who}, price unknown", fmt_tokens(s.subs.tokens()))
        } else if s.subs.usd() == 0.0 {
            format!("+ {} tokens from {who}, free", fmt_tokens(s.subs.tokens()))
        } else if matches!(billing, Some(Billing::Metered)) {
            format!("+ ≈ {} from {who} at API prices", usd(s.subs.usd()))
        } else {
            format!("+ {} from {who}", usd(s.subs.usd()))
        }
    });
    let unpriced = (s.priced() && s.unpriced() > 0).then(|| format!("Not counted: {} tokens without a known price", fmt_tokens(s.unpriced())));
    let mut sources = Vec::new();
    let all: Vec<&ModelSpend> = s.own.models.iter().chain(&s.subs.models).collect();
    let mut agents: Vec<String> = all.iter().filter(|m| m.reported).map(|m| m.agent.display_name()).collect();
    agents.dedup();
    for a in agents {
        if !sources.iter().any(|s: &String| s.starts_with(&a)) {
            sources.push(format!("{a} prices its own usage"));
        }
    }
    let mut providers: Vec<Provider> = Vec::new();
    for p in all.iter().filter_map(|m| m.price).map(|p| p.provider) {
        if !providers.contains(&p) {
            providers.push(p);
        }
    }
    for p in providers {
        let host = p.source().trim_start_matches("https://").split('/').next().unwrap_or_default();
        sources.push(format!("{} prices as of {} · {host}", p.name(), day(p.checked())));
    }
    Some(Breakdown { headline, billing: billing_line, models, subs, unpriced, sources })
}

/// The breakdown as plain text, for `/cost`.
pub fn reply(b: &Breakdown) -> String {
    let mut out = vec![b.headline.clone()];
    if let Some(l) = &b.billing {
        out.push(format!("{l}."));
    }
    for m in &b.models {
        out.push(String::new());
        out.push(format!("**{}** · {}", m.name, m.amount));
        for r in &m.rows {
            out.push(match &r.rate {
                Some(rate) => format!("- {}: {} × {rate}", r.kind, r.tokens),
                None => format!("- {}: {}", r.kind, r.tokens),
            });
        }
    }
    if let Some(s) = &b.subs {
        out.push(String::new());
        out.push(s.clone());
    }
    if let Some(u) = &b.unpriced {
        out.push(u.clone());
    }
    if !b.sources.is_empty() {
        out.push(String::new());
        out.push(b.sources.join(". ") + ".");
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn cache_hits_are_the_share_of_prompt_tokens_read_from_a_cache() {
        use trek_core::{AgentId, TokenUsage};
        let mut s = ThreadSpend::default();
        assert_eq!(CacheHits::of(&s), None, "no prompts yet: nothing to show");
        s.own.add(&AgentId::ClaudeCode, Some("claude-sonnet-4-5"), &TokenUsage { input: 100, output: 50, cache_read: 800, cache_write: 100 }, None, 0);
        s.subs.add(&AgentId::Codex, Some("gpt-5"), &TokenUsage { input: 0, output: 5, cache_read: 0, cache_write: 0 }, None, 0);
        let c = CacheHits::of(&s).unwrap();
        assert_eq!((c.read, c.written, c.fresh), (800, 100, 100));
        assert_eq!(c.label(), "80% cached");
        assert!(c.detail().starts_with("800 of 1K prompt tokens"), "{}", c.detail());
        let nearly = CacheHits { read: 999, written: 0, fresh: 1 };
        assert_eq!(nearly.label(), "99% cached", "not all of it: not 100%");
        assert_eq!(CacheHits { read: 1, written: 0, fresh: 999 }.label(), "< 1% cached");
        assert_eq!(CacheHits { read: 0, written: 0, fresh: 10 }.label(), "0% cached");
    }

    use super::*;
    use trek_core::{AgentId, TokenUsage, UsageCost};

    fn spend(add: &[(AgentId, &str, TokenUsage, Option<UsageCost>)]) -> Spend {
        let mut s = Spend::default();
        for (agent, model, tokens, cost) in add {
            s.add(agent, Some(model), tokens, *cost, 1_790_000_000_000);
        }
        s
    }

    fn claude_turn() -> ThreadSpend {
        let t = TokenUsage { input: 2_400, output: 1_850, cache_read: 182_000, cache_write: 12_600 };
        ThreadSpend { own: spend(&[(AgentId::ClaudeCode, "claude-sonnet-5-5", t, Some(UsageCost::reported(1.234)))]), ..Default::default() }
    }

    #[test]
    fn dollars_read_like_money() {
        assert_eq!(usd(1.234), "$1.23");
        assert_eq!(usd(0.004), "< $0.01");
        assert_eq!(usd(0.0), "$0.00");
        assert_eq!(usd(1234.5), "$1,234.50");
        assert_eq!(rate(2.0), "$2/M");
        assert_eq!(rate(0.2), "$0.20/M");
        assert_eq!(rate(0.075), "$0.075/M");
        assert_eq!(rate(12.5), "$12.50/M");
        assert_eq!((fmt_tokens(850), fmt_tokens(12_345), fmt_tokens(2_000), fmt_tokens(182_000), fmt_tokens(2_100_000)), ("850".into(), "12.3K".into(), "2K".into(), "182K".into(), "2.1M".into()));
    }

    #[test]
    fn the_label_follows_how_the_thread_is_billed() {
        let s = claude_turn();
        let max = Billing::Plan(Some("Claude Max".into()));
        assert_eq!(label(Some(&max), &s).as_deref(), Some("≈ $1.23 at API prices"));
        assert_eq!(label(None, &s).as_deref(), Some("≈ $1.23 at API prices"));
        assert_eq!(label(Some(&Billing::Metered), &s).as_deref(), Some("$1.23"));
        assert_eq!(label(Some(&Billing::Local), &s), None);
        assert_eq!(label(Some(&max), &ThreadSpend::default()), None, "nothing used yet");
        // Billed per token, with a consult that may have run on a plan: only the thread's own
        // spend is a bill.
        let mut consulted = claude_turn();
        consulted.subs = spend(&[(AgentId::Codex, "gpt-5.6-luna", TokenUsage { input: 11_041, output: 5, ..Default::default() }, Some(UsageCost::priced(0.30)))]);
        consulted.sub_threads = 1;
        consulted.consults = true;
        assert_eq!(label(Some(&Billing::Metered), &consulted).as_deref(), Some("$1.23 + ≈ $0.30 from consults"));
        let b = breakdown(Some(&Billing::Metered), &consulted).unwrap();
        assert_eq!((b.headline.as_str(), b.subs.as_deref()), ("$1.23 so far", Some("+ ≈ $0.30 from 1 consult at API prices")));
        assert_eq!(label(Some(&max), &consulted).as_deref(), Some("≈ $1.53 at API prices"));
        // No price for any of it: tokens, honestly.
        let unknown = ThreadSpend { own: spend(&[(AgentId::Acp("devin".into()), "fusion-x", TokenUsage { input: 12_000, output: 300, ..Default::default() }, None)]), ..Default::default() };
        assert_eq!(label(None, &unknown).as_deref(), Some("12.3K tokens · price unknown"));
        // Devin's SWE-2 is free: its tokens, at no cost.
        let free = ThreadSpend { own: spend(&[(AgentId::Acp("devin".into()), "swe-2-high", TokenUsage { input: 100_531, output: 1_122, ..Default::default() }, None)]), ..Default::default() };
        assert_eq!(label(Some(&Billing::Plan(Some("Devin Pro".into()))), &free).as_deref(), Some("102K tokens · free"));
    }

    #[test]
    fn the_breakdown_shows_tokens_by_kind_at_each_price_and_where_prices_come_from() {
        let mut s = claude_turn();
        let codex = TokenUsage { input: 11_041, output: 5, cache_read: 5_888, cache_write: 0 };
        s.subs = spend(&[(AgentId::Codex, "gpt-5.6-luna", codex, Some(UsageCost::priced(0.30)))]);
        s.sub_threads = 2;
        s.consults = true;
        let b = breakdown(Some(&Billing::Plan(Some("Claude Max".into()))), &s).unwrap();
        assert_eq!(b.headline, "≈ $1.53 at API prices", "its consults' share included");
        assert_eq!(b.billing.as_deref(), Some("Included in your Claude Max plan"));
        assert_eq!(b.models.len(), 1);
        let m = &b.models[0];
        assert_eq!((m.name.as_str(), m.amount.as_str()), ("Claude Sonnet 5.5", "$1.23"));
        assert_eq!(
            m.rows,
            vec![
                Row { kind: "Input", tokens: "2.4K".into(), rate: Some("$2/M".into()) },
                Row { kind: "Cache write", tokens: "12.6K".into(), rate: Some("$2.50/M (1h: $4/M)".into()) },
                Row { kind: "Cache read", tokens: "182K".into(), rate: Some("$0.20/M".into()) },
                Row { kind: "Output", tokens: "1.9K".into(), rate: Some("$10/M".into()) },
            ]
        );
        assert_eq!(b.subs.as_deref(), Some("+ $0.30 from 2 consults"));
        assert_eq!(b.unpriced, None);
        assert_eq!(
            b.sources,
            vec![
                "Claude Code prices its own usage".to_string(),
                "Anthropic prices as of Oct 3, 2026 · platform.claude.com".to_string(),
                "OpenAI prices as of Oct 3, 2026 · developers.openai.com".to_string(),
            ]
        );
        let text = reply(&b);
        assert!(text.starts_with("≈ $1.53 at API prices\nIncluded in your Claude Max plan."), "{text}");
        assert!(text.contains("- Cache read: 182K × $0.20/M"));
    }

    #[test]
    fn tokens_without_a_price_are_left_out_and_said_so() {
        let t = TokenUsage { input: 5_000, output: 500, ..Default::default() };
        let s = ThreadSpend {
            own: spend(&[(AgentId::Codex, "gpt-5.6-luna", t, None), (AgentId::Codex, "gpt-9-secret", t, None)]),
            ..Default::default()
        };
        let b = breakdown(Some(&Billing::Metered), &s).unwrap();
        // Priced from the table: 5,000 × $0.20 + 500 × $1.20, per million.
        assert_eq!(b.headline, "< $0.01 so far");
        assert_eq!(b.unpriced.as_deref(), Some("Not counted: 5.5K tokens without a known price"));
        let unknown = b.models.iter().find(|m| m.name == "gpt-9-secret").unwrap();
        assert_eq!(unknown.amount, "price unknown");
        assert!(unknown.rows.iter().all(|r| r.rate.is_none()));
        assert_eq!(b.sources, vec!["OpenAI prices as of Oct 3, 2026 · developers.openai.com".to_string()]);
    }
}
