//! What models cost through their providers' APIs, for the estimate under the composer and in
//! Basecamp. Each provider's prices come from its own pricing page (`Provider::source`), checked
//! on the date given (`Provider::checked`). Models that aren't here have no known price: Trek
//! shows their tokens, never a guess.
//!
//! Prices are per request where providers price them so: a long-context tier applies to a
//! request whose prompt is over the threshold, which only the request's own usage can tell.
//! Agents that report usage per request (Codex, direct providers) are priced as they go;
//! totals priced afterwards (`estimate`) use the standard tier.

use crate::types::{AgentId, TokenUsage, UsageCost};
use chrono::{NaiveDate, TimeZone, Utc};

/// Dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    /// Fresh input (not read from or written to the prompt cache).
    pub input: f64,
    /// Output, reasoning included.
    pub output: f64,
    /// Writing the prompt cache: Anthropic's 5-minute writes, OpenAI's writes. Providers that
    /// don't charge for writes have the input price here.
    pub cache_write: f64,
    /// Anthropic's 1-hour cache writes, priced apart from 5-minute ones.
    pub cache_write_1h: Option<f64>,
    pub cache_read: f64,
}

impl Rates {
    /// Writes cost what input does (no write charge, or no prompt cache to speak of).
    const fn simple(input: f64, cache_read: f64, output: f64) -> Rates {
        Rates { input, output, cache_write: input, cache_write_1h: None, cache_read }
    }

    /// Anthropic's: 5-minute writes at 1.25× input, 1-hour writes at 2×.
    const fn anthropic(input: f64, cache_read: f64, output: f64) -> Rates {
        Rates { input, output, cache_write: input * 1.25, cache_write_1h: Some(input * 2.0), cache_read }
    }

    const fn openai(input: f64, cache_read: f64, cache_write: f64, output: f64) -> Rates {
        Rates { input, output, cache_write, cache_write_1h: None, cache_read }
    }

    fn scaled(&self, by: f64) -> Rates {
        Rates {
            input: self.input * by,
            output: self.output * by,
            cache_write: self.cache_write * by,
            cache_write_1h: self.cache_write_1h.map(|r| r * by),
            cache_read: self.cache_read * by,
        }
    }

    /// What `tokens` cost at these rates; `cache_write_1h` of the cache writes were 1-hour ones.
    pub fn cost(&self, tokens: &TokenUsage, cache_write_1h: u64) -> f64 {
        let long_writes = cache_write_1h.min(tokens.cache_write);
        let per = |n: u64, rate: f64| n as f64 * rate / 1_000_000.0;
        per(tokens.input, self.input)
            + per(tokens.output, self.output)
            + per(tokens.cache_read, self.cache_read)
            + per(tokens.cache_write - long_writes, self.cache_write)
            + per(long_writes, self.cache_write_1h.unwrap_or(self.cache_write))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Anthropic,
    OpenAi,
    Google,
    Xai,
    /// Devin's own per-token prices for the models it serves (its CLI lists them).
    Devin,
}

impl Provider {
    pub fn name(&self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic",
            Provider::OpenAi => "OpenAI",
            Provider::Google => "Google",
            Provider::Xai => "xAI",
            Provider::Devin => "Devin",
        }
    }

    /// Where the prices were read.
    pub fn source(&self) -> &'static str {
        match self {
            Provider::Anthropic => "https://platform.claude.com/docs/en/about-claude/pricing",
            Provider::OpenAi => "https://developers.openai.com/api/docs/pricing",
            Provider::Google => "https://ai.google.dev/gemini-api/docs/pricing",
            Provider::Xai => "https://docs.x.ai/docs/models",
            // `devin models list` prints them; the page explains the models.
            Provider::Devin => "https://docs.devin.ai/cli/models",
        }
    }

    /// When they were last checked against the source (YYYY-MM-DD).
    pub fn checked(&self) -> &'static str {
        "2026-10-03"
    }
}

/// A model's API prices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    /// The provider's model id.
    pub model: &'static str,
    pub name: &'static str,
    pub provider: Provider,
    pub rates: Rates,
    /// Requests with at least `.0` prompt tokens (input, cache reads and writes) cost `.1` instead.
    pub long: Option<(u64, Rates)>,
    /// Fast mode (Anthropic) or Fast processing (OpenAI) costs this many times the price.
    pub fast: Option<f64>,
    /// The first day (UTC, YYYY-MM-DD) these prices apply, for prices announced ahead.
    pub from: Option<&'static str>,
}

impl Price {
    const fn new(model: &'static str, name: &'static str, provider: Provider, rates: Rates) -> Price {
        Price { model, name, provider, rates, long: None, fast: None, from: None }
    }

    const fn long(mut self, from_tokens: u64, rates: Rates) -> Price {
        self.long = Some((from_tokens, rates));
        self
    }

    const fn fast(mut self, by: f64) -> Price {
        self.fast = Some(by);
        self
    }

    const fn from(mut self, day: &'static str) -> Price {
        self.from = Some(day);
        self
    }

    /// The rates one request is billed at: by its prompt's size, and its speed.
    pub fn rates_for(&self, tokens: &TokenUsage, fast: bool) -> Rates {
        let prompt = tokens.input + tokens.cache_read + tokens.cache_write;
        let rates = match self.long {
            Some((from, long)) if prompt >= from => long,
            _ => self.rates,
        };
        match self.fast.filter(|_| fast) {
            Some(by) => rates.scaled(by),
            None => rates,
        }
    }

    /// What one request that used `tokens` costs.
    pub fn request(&self, tokens: &TokenUsage, cache_write_1h: u64, fast: bool) -> f64 {
        self.rates_for(tokens, fast).cost(tokens, cache_write_1h)
    }
}

use Provider::*;

/// OpenAI's long context: over 272K prompt tokens.
const OPENAI_LONG: u64 = 272_001;
/// Gemini's: prompts over 200K tokens.
const GEMINI_LONG: u64 = 200_001;
/// xAI's: prompts of 200K tokens and more.
const XAI_LONG: u64 = 200_000;

pub static PRICES: &[Price] = &[
    // Anthropic. No long-context premium on current models: the 1M window is priced as one.
    Price::new("claude-fable-5-1", "Claude Fable 5.1", Anthropic, Rates::anthropic(10.0, 0.25, 50.0)),
    Price::new("claude-mythos-5-1", "Claude Mythos 5.1", Anthropic, Rates::anthropic(10.0, 0.25, 50.0)),
    Price::new("claude-fable-5", "Claude Fable 5", Anthropic, Rates::anthropic(10.0, 1.0, 50.0)),
    Price::new("claude-mythos-5", "Claude Mythos 5", Anthropic, Rates::anthropic(10.0, 1.0, 50.0)),
    Price::new("claude-opus-5-5", "Claude Opus 5.5", Anthropic, Rates::anthropic(4.0, 0.20, 20.0)).fast(2.0),
    Price::new("claude-opus-5", "Claude Opus 5", Anthropic, Rates::anthropic(5.0, 0.50, 25.0)).fast(2.0),
    Price::new("claude-opus-4-8", "Claude Opus 4.8", Anthropic, Rates::anthropic(5.0, 0.50, 25.0)).fast(2.0),
    Price::new("claude-opus-4-7", "Claude Opus 4.7", Anthropic, Rates::anthropic(5.0, 0.50, 25.0)),
    Price::new("claude-opus-4-6", "Claude Opus 4.6", Anthropic, Rates::anthropic(5.0, 0.50, 25.0)),
    Price::new("claude-opus-4-5", "Claude Opus 4.5", Anthropic, Rates::anthropic(5.0, 0.50, 25.0)),
    Price::new("claude-opus-4-1", "Claude Opus 4.1", Anthropic, Rates::anthropic(15.0, 1.50, 75.0)),
    Price::new("claude-opus-4", "Claude Opus 4", Anthropic, Rates::anthropic(15.0, 1.50, 75.0)),
    Price::new("claude-sonnet-5-5", "Claude Sonnet 5.5", Anthropic, Rates::anthropic(2.0, 0.20, 10.0)),
    Price::new("claude-sonnet-5", "Claude Sonnet 5", Anthropic, Rates::anthropic(2.0, 0.20, 10.0)),
    Price::new("claude-sonnet-4-6", "Claude Sonnet 4.6", Anthropic, Rates::anthropic(3.0, 0.30, 15.0)),
    Price::new("claude-sonnet-4-5", "Claude Sonnet 4.5", Anthropic, Rates::anthropic(3.0, 0.30, 15.0)),
    Price::new("claude-sonnet-4", "Claude Sonnet 4", Anthropic, Rates::anthropic(3.0, 0.30, 15.0)),
    Price::new("claude-haiku-4-5", "Claude Haiku 4.5", Anthropic, Rates::anthropic(1.0, 0.10, 5.0)),
    Price::new("claude-haiku-3-5", "Claude Haiku 3.5", Anthropic, Rates::anthropic(0.80, 0.08, 4.0)),
    // OpenAI: standard processing; Fast processing (formerly Priority) as a multiple of it.
    Price::new("gpt-6-astra", "GPT-6 Astra", OpenAi, Rates::openai(10.0, 1.0, 12.5, 50.0)).long(OPENAI_LONG, Rates::openai(20.0, 2.0, 25.0, 75.0)).fast(2.0),
    Price::new("gpt-6.1-sol", "GPT-6.1 Sol", OpenAi, Rates::openai(2.0, 0.10, 2.5, 10.0)).long(OPENAI_LONG, Rates::openai(4.0, 0.20, 5.0, 15.0)).fast(2.0),
    Price::new("gpt-6-sol", "GPT-6 Sol", OpenAi, Rates::openai(2.0, 0.20, 2.5, 10.0)).long(OPENAI_LONG, Rates::openai(4.0, 0.40, 5.0, 15.0)).fast(2.0),
    Price::new("gpt-6-luna", "GPT-6 Luna", OpenAi, Rates::openai(0.10, 0.01, 0.125, 0.50)).long(OPENAI_LONG, Rates::openai(0.20, 0.02, 0.25, 0.75)).fast(2.0),
    Price::new("gpt-5.6-sol", "GPT-5.6 Sol", OpenAi, Rates::openai(4.0, 0.40, 5.0, 20.0)).long(OPENAI_LONG, Rates::openai(8.0, 0.80, 10.0, 30.0)).fast(2.0),
    Price::new("gpt-5.6-terra", "GPT-5.6 Terra", OpenAi, Rates::openai(2.0, 0.20, 2.5, 12.0)).long(OPENAI_LONG, Rates::openai(4.0, 0.40, 5.0, 18.0)).fast(2.0),
    Price::new("gpt-5.6-luna", "GPT-5.6 Luna", OpenAi, Rates::openai(0.20, 0.02, 0.25, 1.20)).long(OPENAI_LONG, Rates::openai(0.40, 0.04, 0.50, 1.80)).fast(2.0),
    Price::new("gpt-5.5", "GPT-5.5", OpenAi, Rates::simple(5.0, 0.50, 30.0)).long(OPENAI_LONG, Rates::simple(10.0, 1.0, 45.0)).fast(2.5),
    Price::new("gpt-5.4", "GPT-5.4", OpenAi, Rates::simple(2.5, 0.25, 15.0)).long(OPENAI_LONG, Rates::simple(5.0, 0.50, 22.5)).fast(2.0),
    Price::new("gpt-5.4-mini", "GPT-5.4 mini", OpenAi, Rates::simple(0.75, 0.075, 4.5)).fast(2.0),
    Price::new("gpt-5.4-nano", "GPT-5.4 nano", OpenAi, Rates::simple(0.20, 0.02, 1.25)),
    Price::new("gpt-5.2", "GPT-5.2", OpenAi, Rates::simple(1.75, 0.175, 14.0)).fast(2.0),
    Price::new("gpt-5.1", "GPT-5.1", OpenAi, Rates::simple(1.25, 0.125, 10.0)).fast(2.0),
    Price::new("gpt-5", "GPT-5", OpenAi, Rates::simple(1.25, 0.125, 10.0)).fast(2.0),
    Price::new("gpt-5-mini", "GPT-5 mini", OpenAi, Rates::simple(0.25, 0.025, 2.0)),
    Price::new("gpt-5-nano", "GPT-5 nano", OpenAi, Rates::simple(0.05, 0.005, 0.40)),
    Price::new("gpt-4.1", "GPT-4.1", OpenAi, Rates::simple(2.0, 0.50, 8.0)),
    Price::new("gpt-4.1-mini", "GPT-4.1 mini", OpenAi, Rates::simple(0.40, 0.10, 1.60)),
    Price::new("gpt-4o", "GPT-4o", OpenAi, Rates::simple(2.5, 1.25, 10.0)),
    Price::new("gpt-4o-mini", "GPT-4o mini", OpenAi, Rates::simple(0.15, 0.075, 0.60)),
    Price::new("o3", "o3", OpenAi, Rates::simple(2.0, 0.50, 8.0)),
    Price::new("o4-mini", "o4-mini", OpenAi, Rates::simple(1.10, 0.275, 4.40)),
    // Google. Implicit caching has no write charge; storage of explicit caches isn't per token.
    // The 3.6–3.8 Flash prices double on January 1, 2027, as the page announces.
    Price::new("gemini-3.8-flash", "Gemini 3.8 Flash", Google, Rates::simple(0.75, 0.075, 3.75)),
    Price::new("gemini-3.8-flash", "Gemini 3.8 Flash", Google, Rates::simple(1.50, 0.15, 7.50)).from("2027-01-01"),
    Price::new("gemini-3.7-flash", "Gemini 3.7 Flash", Google, Rates::simple(0.75, 0.075, 3.75)),
    Price::new("gemini-3.7-flash", "Gemini 3.7 Flash", Google, Rates::simple(1.50, 0.15, 7.50)).from("2027-01-01"),
    Price::new("gemini-3.6-flash", "Gemini 3.6 Flash", Google, Rates::simple(0.75, 0.075, 3.75)),
    Price::new("gemini-3.6-flash", "Gemini 3.6 Flash", Google, Rates::simple(1.50, 0.15, 7.50)).from("2027-01-01"),
    Price::new("gemini-3.5-flash", "Gemini 3.5 Flash", Google, Rates::simple(1.50, 0.15, 9.0)),
    Price::new("gemini-3.5-flash-lite", "Gemini 3.5 Flash-Lite", Google, Rates::simple(0.30, 0.03, 2.50)),
    Price::new("gemini-3.1-flash-lite", "Gemini 3.1 Flash-Lite", Google, Rates::simple(0.25, 0.025, 1.50)),
    Price::new("gemini-3.1-pro-preview", "Gemini 3.1 Pro", Google, Rates::simple(2.0, 0.20, 12.0)).long(GEMINI_LONG, Rates::simple(4.0, 0.40, 18.0)),
    Price::new("gemini-3-flash-preview", "Gemini 3 Flash", Google, Rates::simple(0.50, 0.05, 3.0)),
    Price::new("gemini-2.5-pro", "Gemini 2.5 Pro", Google, Rates::simple(1.25, 0.125, 10.0)).long(GEMINI_LONG, Rates::simple(2.50, 0.25, 15.0)),
    Price::new("gemini-2.5-flash", "Gemini 2.5 Flash", Google, Rates::simple(0.30, 0.03, 2.50)),
    Price::new("gemini-2.5-flash-lite", "Gemini 2.5 Flash-Lite", Google, Rates::simple(0.10, 0.01, 0.40)),
    // xAI.
    Price::new("grok-4.7", "Grok 4.7", Xai, Rates::simple(2.0, 0.50, 6.0)).long(XAI_LONG, Rates::simple(4.0, 1.0, 12.0)),
    Price::new("grok-4.6", "Grok 4.6", Xai, Rates::simple(2.0, 0.50, 6.0)).long(XAI_LONG, Rates::simple(4.0, 1.0, 12.0)),
    Price::new("grok-4.5", "Grok 4.5", Xai, Rates::simple(2.0, 0.30, 6.0)).long(XAI_LONG, Rates::simple(4.0, 0.60, 12.0)),
    Price::new("grok-4.3", "Grok 4.3", Xai, Rates::simple(1.25, 0.20, 2.50)).long(XAI_LONG, Rates::simple(2.50, 0.40, 5.0)),
    Price::new("grok-4.20-0309-reasoning", "Grok 4.20", Xai, Rates::simple(1.25, 0.20, 2.50)).long(XAI_LONG, Rates::simple(2.50, 0.40, 5.0)),
    Price::new("grok-4.20-0309-non-reasoning", "Grok 4.20", Xai, Rates::simple(1.25, 0.20, 2.50)).long(XAI_LONG, Rates::simple(2.50, 0.40, 5.0)),
    Price::new("grok-4.20-multi-agent-0309", "Grok 4.20 Multi-agent", Xai, Rates::simple(1.25, 0.20, 2.50)).long(XAI_LONG, Rates::simple(2.50, 0.40, 5.0)),
    Price::new("grok-build-0.1", "Grok Build", Xai, Rates::simple(1.0, 0.20, 2.0)).long(XAI_LONG, Rates::simple(2.0, 0.40, 4.0)),
    // Devin's own models, and the ones it prices apart from their makers.
    Price::new("adaptive", "Adaptive", Devin, Rates::simple(0.50, 0.10, 2.0)),
    Price::new("swe-2", "SWE-2", Devin, Rates::simple(0.0, 0.0, 0.0)),
    Price::new("swe-1.7", "SWE-1.7", Devin, Rates::simple(0.50, 0.20, 2.50)),
    Price::new("swe-1.7-lightning", "SWE-1.7 Lightning", Devin, Rates::simple(2.50, 1.0, 12.50)),
    Price::new("kimi-k3", "Kimi K3", Devin, Rates::simple(3.0, 0.30, 15.0)),
    Price::new("glm-5.2", "GLM-5.2", Devin, Rates::simple(1.40, 0.26, 4.40)),
    Price::new("glm-5.3", "GLM-5.3", Devin, Rates::simple(1.40, 0.26, 4.40)),
    Price::new("glm-5.3-flash", "GLM-5.3 Flash", Devin, Rates::simple(0.15, 0.03, 0.50)),
    Price::new("deepseek-v4-flash", "DeepSeek V4 Flash", Devin, Rates::simple(0.14, 0.03, 0.28)),
    Price::new("deepseek-v4.1-flash", "DeepSeek V4.1 Flash", Devin, Rates::simple(0.22, 0.01, 0.66)),
];

/// Other names agents give priced models.
const ALIASES: &[(&str, &str)] = &[("claude-5-fable", "claude-fable-5")];

/// Models Trek's catalog offers that have no API price: the mock agents.
pub const UNPRICED: &[&str] = &[crate::catalog::MOCK_PROVIDER, crate::catalog::MOCK_RELAY_PROVIDER];

/// Words agents append to a model id for its effort or speed; they don't change its price.
const EFFORT_SUFFIXES: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max", "thinking", "1m"];

/// A model id as prices are looked up: lower case, dashes for dots, without a provider prefix
/// (`anthropic/…`, `us.anthropic.…`), a context tag (`[1m]`), or a date (`-20251001`).
fn canonical(model: &str) -> String {
    let mut m = model.trim().to_ascii_lowercase();
    if let Some(i) = m.find('[') {
        m.truncate(i);
    }
    if let Some(i) = m.find('@') {
        m.truncate(i);
    }
    if let Some(i) = m.rfind('/') {
        m = m[i + 1..].to_string();
    }
    if let Some(i) = m.find("claude-") {
        m = m[i..].to_string();
    }
    let mut m = m.replace(['.', '_', ':'], "-");
    // A dated snapshot: `-20251001`, `-2025-10-01`, Bedrock's `-v1-0`.
    for suffix in ["-v1-0", "-v2-0"] {
        if let Some(s) = m.strip_suffix(suffix) {
            m = s.to_string();
        }
    }
    let parts: Vec<&str> = m.split('-').collect();
    let date = |p: &[&str]| -> usize {
        match p {
            [.., y, mo, d] if y.len() == 4 && mo.len() == 2 && d.len() == 2 && [y, mo, d].iter().all(|s| s.chars().all(|c| c.is_ascii_digit())) => 3,
            [.., d] if d.len() == 8 && d.chars().all(|c| c.is_ascii_digit()) => 1,
            _ => 0,
        }
    };
    let cut = date(&parts);
    parts[..parts.len() - cut].join("-")
}

/// The price for `model` as `agent` serves it on `day` (UTC), and whether the id asks for fast
/// processing (`-fast`, `-priority`). Devin's own prices come first for Devin's threads.
pub fn price_on(model: &str, agent: &AgentId, day: NaiveDate) -> Option<(&'static Price, bool)> {
    let mut key = canonical(model);
    let mut fast = false;
    loop {
        if let Some(rest) = key.strip_suffix("-fast").or_else(|| key.strip_suffix("-priority")) {
            fast = true;
            key = rest.to_string();
            continue;
        }
        let name = ALIASES.iter().find(|(a, _)| *a == key).map_or(key.as_str(), |(_, to)| to);
        if let Some(p) = find(name, agent, day) {
            return Some((p, fast));
        }
        let (rest, last) = key.rsplit_once('-')?;
        if !EFFORT_SUFFIXES.contains(&last) {
            return None;
        }
        key = rest.to_string();
    }
}

fn find(key: &str, agent: &AgentId, day: NaiveDate) -> Option<&'static Price> {
    // A model on this Mac costs nothing per token, whatever it's called.
    if let AgentId::Direct(p) = agent
        && crate::catalog::direct_provider(p).is_some_and(|p| p.local)
    {
        return None;
    }
    let devin = matches!(agent, AgentId::Acp(a) if a == "devin");
    let applies = |p: &&Price| p.from.and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).is_none_or(|from| day >= from);
    let matching = || PRICES.iter().filter(|p| canonical(p.model) == key).filter(applies);
    // The latest entry that applies; Devin's own for Devin, everyone else's for the rest.
    let pick = |own: bool| matching().rfind(|p| (p.provider == Provider::Devin) == own);
    if devin { pick(true).or_else(|| pick(false)) } else { pick(false) }
}

/// `price_on` today.
pub fn price(model: &str, agent: &AgentId) -> Option<(&'static Price, bool)> {
    price_on(model, agent, Utc::now().date_naive())
}

fn day_of(at_ms: i64) -> NaiveDate {
    Utc.timestamp_millis_opt(at_ms).single().map_or_else(|| Utc::now().date_naive(), |t| t.date_naive())
}

/// What one request of `model` that used `tokens` cost (with `cache_write_1h` of its cache
/// writes for an hour, and `fast` as the session ran), priced by Trek.
pub fn request(model: &str, agent: &AgentId, tokens: &TokenUsage, cache_write_1h: u64, fast: bool) -> Option<UsageCost> {
    let (p, fast_id) = price(model, agent)?;
    Some(UsageCost::priced(p.request(tokens, cache_write_1h, fast || fast_id)))
}

/// What `tokens` on `model` cost at standard rates on the day of `at_ms`: for totals over many
/// requests, where the long-context tier can't be told.
pub fn estimate(model: &str, agent: &AgentId, tokens: &TokenUsage, at_ms: i64) -> Option<f64> {
    let (p, fast) = price_on(model, agent, day_of(at_ms))?;
    let rates = if fast { p.rates.scaled(p.fast.unwrap_or(1.0)) } else { p.rates };
    Some(rates.cost(tokens, 0))
}

/// What a thread (or a day) spent, by model: tokens, and their cost where it's known.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spend {
    pub models: Vec<ModelSpend>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelSpend {
    /// The model's priced id, else as the agent named it (`None`: it named none).
    pub model: Option<String>,
    pub agent: AgentId,
    pub price: Option<&'static Price>,
    pub tokens: TokenUsage,
    /// Dollars, for the tokens with a cost.
    pub usd: f64,
    /// Tokens no cost is known for.
    pub unpriced: u64,
    /// Some of the cost is the agent's own figure.
    pub reported: bool,
}

impl Spend {
    /// Count one report: `cost` as it was recorded, else priced now from the table.
    pub fn add(&mut self, agent: &AgentId, model: Option<&str>, tokens: &TokenUsage, cost: Option<UsageCost>, at_ms: i64) {
        let price = model.and_then(|m| price_on(m, agent, day_of(at_ms))).map(|(p, _)| p);
        let cost = cost.or_else(|| model.and_then(|m| estimate(m, agent, tokens, at_ms)).map(UsageCost::priced));
        let key = price.map(|p| p.model.to_string()).or_else(|| model.map(String::from));
        let i = match self.models.iter().position(|m| m.model == key && m.agent == *agent) {
            Some(i) => i,
            None => {
                self.models.push(ModelSpend { model: key, agent: agent.clone(), price, tokens: TokenUsage::default(), usd: 0.0, unpriced: 0, reported: false });
                self.models.len() - 1
            }
        };
        let m = &mut self.models[i];
        m.tokens.add(tokens);
        match cost {
            Some(c) => {
                m.usd += c.usd;
                m.reported |= c.reported;
            }
            None => m.unpriced += tokens.total(),
        }
    }

    pub fn merge(&mut self, other: &Spend) {
        for o in &other.models {
            match self.models.iter_mut().find(|m| m.model == o.model && m.agent == o.agent) {
                Some(m) => {
                    m.tokens.add(&o.tokens);
                    m.usd += o.usd;
                    m.unpriced += o.unpriced;
                    m.reported |= o.reported;
                }
                None => self.models.push(o.clone()),
            }
        }
    }

    /// Dollars for the tokens with a known cost.
    pub fn usd(&self) -> f64 {
        self.models.iter().map(|m| m.usd).sum()
    }

    pub fn tokens(&self) -> u64 {
        self.models.iter().map(|m| m.tokens.total()).sum()
    }

    /// Tokens without a known cost.
    pub fn unpriced(&self) -> u64 {
        self.models.iter().map(|m| m.unpriced).sum()
    }

    /// Some tokens have a cost (perhaps zero: a free model).
    pub fn priced(&self) -> bool {
        self.models.iter().any(|m| m.tokens.total() > m.unpriced || m.usd > 0.0)
    }

    pub fn is_empty(&self) -> bool {
        self.tokens() == 0 && self.usd() == 0.0
    }

    /// Most expensive first, then most tokens.
    pub fn sorted(&self) -> Vec<&ModelSpend> {
        let mut out: Vec<&ModelSpend> = self.models.iter().filter(|m| m.tokens.total() > 0 || m.usd > 0.0).collect();
        out.sort_by(|a, b| b.usd.total_cmp(&a.usd).then(b.tokens.total().cmp(&a.tokens.total())));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn every_catalog_model_has_a_price_or_is_known_not_to() {
        let agents = [AgentId::ClaudeCode, AgentId::Codex, AgentId::Direct("anthropic".into()), AgentId::Direct("openai".into())];
        for agent in &agents {
            for m in catalog::default_models(agent) {
                assert!(price(&m.id, agent).is_some(), "{} ({}) has no price", m.id, agent.key());
            }
        }
        for provider in UNPRICED {
            let agent = AgentId::Direct(provider.to_string());
            for m in catalog::default_models(&agent) {
                assert!(price(&m.id, &agent).is_none(), "{} is marked unpriced but has a price", m.id);
            }
        }
        // Local models are free to run: nothing to price, nothing offered by default.
        for local in catalog::DIRECT_PROVIDERS.iter().filter(|p| p.local) {
            assert!(catalog::default_models(&AgentId::Direct(local.id.into())).is_empty());
        }
    }

    #[test]
    fn every_price_is_sane_and_cites_its_source() {
        for p in PRICES {
            let r = p.rates;
            assert!(r.input >= 0.0 && r.output >= r.input && r.cache_read <= r.input, "{}", p.model);
            assert!(r.cache_write >= r.input, "{}", p.model);
            if let Some((from, long)) = p.long {
                assert!(from > 100_000 && long.input > r.input, "{}", p.model);
            }
            assert!(p.provider.source().starts_with("https://"));
            assert!(NaiveDate::parse_from_str(p.provider.checked(), "%Y-%m-%d").is_ok());
        }
    }

    #[test]
    fn model_ids_resolve_however_agents_write_them() {
        let claude = AgentId::ClaudeCode;
        let id = |m: &str, agent: &AgentId| price(m, agent).map(|(p, fast)| (p.model, fast));
        assert_eq!(id("claude-haiku-4-5-20251001", &claude), Some(("claude-haiku-4-5", false)));
        assert_eq!(id("claude-opus-5-5[1m]", &claude), Some(("claude-opus-5-5", false)));
        assert_eq!(id("anthropic/claude-sonnet-5-5", &AgentId::OpenCode), Some(("claude-sonnet-5-5", false)));
        assert_eq!(id("us.anthropic.claude-sonnet-4-5-20250929-v1:0", &claude), Some(("claude-sonnet-4-5", false)));
        assert_eq!(id("gpt-5.6-luna", &AgentId::Codex), Some(("gpt-5.6-luna", false)));
        // Devin's ids carry the effort and speed.
        let devin = AgentId::Acp("devin".into());
        assert_eq!(id("gpt-5-6-luna-medium", &devin), Some(("gpt-5.6-luna", false)));
        assert_eq!(id("claude-opus-5-5-high-fast", &devin), Some(("claude-opus-5-5", true)));
        assert_eq!(id("gpt-6-astra-xhigh-priority", &devin), Some(("gpt-6-astra", true)));
        assert_eq!(id("swe-2-high", &devin), Some(("swe-2", false)));
        assert_eq!(id("claude-5-fable-medium", &devin), Some(("claude-fable-5", false)));
        // Devin's own prices are Devin's: elsewhere a SWE model has none.
        assert_eq!(id("swe-2-high", &AgentId::OpenCode), None);
        // No guessing: an unknown model, or a known one with a word that changes what it is.
        assert_eq!(id("gpt-5.6-luna-mini", &AgentId::Codex), None);
        assert_eq!(id("mock-swift", &AgentId::Direct("mock".into())), None);
        assert_eq!(id("gpt-5.6-luna", &AgentId::Direct("ollama".into())), None, "local models are free to run");
        assert_eq!(id("fusion-claude-opus-5-5-high-sidekick-swe-2-medium", &devin), None);
        assert_eq!(id("gemini-2.5-pro", &AgentId::Direct("google".into())), Some(("gemini-2.5-pro", false)));
        assert_eq!(id("models/gemini-2.5-flash", &AgentId::Acp("gemini".into())), Some(("gemini-2.5-flash", false)));
        assert_eq!(id("grok-4.7", &AgentId::Acp("grok".into())), Some(("grok-4.7", false)));
    }

    #[test]
    fn announced_prices_apply_from_their_day() {
        let agent = AgentId::Direct("google".into());
        assert_eq!(price_on("gemini-3.8-flash", &agent, day("2026-12-31")).unwrap().0.rates.input, 0.75);
        assert_eq!(price_on("gemini-3.8-flash", &agent, day("2027-01-01")).unwrap().0.rates.input, 1.50);
    }

    #[test]
    fn claude_code_turn_prices_like_claude_code() {
        // A real `result` from Claude Code 2.1.288 on claude-haiku-4-5: its main model's turn
        // (all cache writes 1-hour) and a title call on the dated id. Claude Code said
        // costUSD 0.0197463 and 0.000942, total_cost_usd 0.0206883.
        let (haiku, _) = price("claude-haiku-4-5", &AgentId::ClaudeCode).unwrap();
        let turn = TokenUsage { input: 10, output: 44, cache_read: 13_803, cache_write: 9_068 };
        assert!(close(haiku.request(&turn, 9_068, false), 0.0197463));
        let title = TokenUsage { input: 897, output: 9, ..Default::default() };
        assert!(close(haiku.request(&title, 0, false), 0.000942));
        // 10×$1 + 44×$5 + 13,803×$0.10 + 9,068×$2 (1-hour writes), per million.
        assert!(close(haiku.request(&turn, 0, false), 0.00001 + 0.00022 + 0.0013803 + 0.011335), "5-minute writes at $1.25");
    }

    #[test]
    fn codex_requests_price_their_tier() {
        let (luna, _) = price("gpt-5.6-luna", &AgentId::Codex).unwrap();
        // A real `token_count` (Codex 0.160): 16,929 input of which 5,888 cached, 5 output.
        let t = TokenUsage { input: 16_929 - 5_888, output: 5, cache_read: 5_888, cache_write: 0 };
        // 11,041×$0.20 + 5×$1.20 + 5,888×$0.02, per million.
        assert!(close(luna.request(&t, 0, false), 0.0022082 + 0.000006 + 0.00011776));
        assert!(close(luna.request(&t, 0, true), 2.0 * (0.0022082 + 0.000006 + 0.00011776)), "Fast is twice the price");
        // Over 272K prompt tokens, the whole request is long context.
        let long = TokenUsage { input: 300_000, output: 1_000, cache_read: 0, cache_write: 0 };
        assert!(close(luna.request(&long, 0, false), 300_000.0 * 0.40 / 1e6 + 1_000.0 * 1.80 / 1e6));
        let edge = TokenUsage { input: 272_000, ..Default::default() };
        assert!(close(luna.request(&edge, 0, false), 272_000.0 * 0.20 / 1e6));
    }

    #[test]
    fn gemini_and_grok_long_context() {
        let (pro, _) = price("gemini-2.5-pro", &AgentId::Direct("google".into())).unwrap();
        assert!(close(pro.request(&TokenUsage { input: 200_000, output: 100, ..Default::default() }, 0, false), 0.25 + 0.001));
        assert!(close(pro.request(&TokenUsage { input: 200_001, output: 100, ..Default::default() }, 0, false), 200_001.0 * 2.5 / 1e6 + 0.0015));
        let (grok, _) = price("grok-4.7", &AgentId::Direct("xai".into())).unwrap();
        assert!(close(grok.request(&TokenUsage { input: 199_000, cache_read: 1_000, ..Default::default() }, 0, false), 199_000.0 * 4.0 / 1e6 + 1_000.0 * 1.0 / 1e6), "200K and over");
    }

    #[test]
    fn spend_adds_up_by_model_and_keeps_unpriced_tokens_apart() {
        let mut s = Spend::default();
        let claude = AgentId::ClaudeCode;
        let t = TokenUsage { input: 1_000, output: 1_000, ..Default::default() };
        s.add(&claude, Some("claude-sonnet-5-5"), &t, Some(UsageCost::reported(0.5)), 0);
        s.add(&claude, Some("claude-sonnet-5-5-20260901"), &t, None, 0);
        s.add(&AgentId::Acp("devin".into()), Some("fusion-x"), &t, None, 0);
        assert_eq!(s.models.len(), 2, "one row per priced model");
        let sonnet = &s.models[0];
        assert_eq!(sonnet.model.as_deref(), Some("claude-sonnet-5-5"));
        assert!(close(sonnet.usd, 0.5 + 0.002 + 0.01) && sonnet.reported);
        assert_eq!(s.unpriced(), 2_000);
        assert_eq!(s.tokens(), 6_000);
        assert!(s.priced());
        let mut total = Spend::default();
        total.merge(&s);
        total.merge(&s);
        assert!(close(total.usd(), 2.0 * s.usd()));
        assert_eq!(total.sorted()[0].model.as_deref(), Some("claude-sonnet-5-5"));
        let mut free = Spend::default();
        free.add(&AgentId::Acp("devin".into()), Some("swe-2-high"), &t, None, 0);
        assert!(free.priced() && free.usd() == 0.0, "a free model is priced, at nothing");
    }
}
