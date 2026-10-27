//! Chromium's declarativeNetRequest rules (an extension's `rule_resources`) as WebKit content
//! rules (Safari's content-blocker JSON), and a filter list's element-hiding filters as
//! `css-display-none` rules.
//!
//! WebKit can't do all DNR says, so each rule converts or is skipped, and counted:
//! - `urlFilter` (`||`, `|`, `^`, `*`) and `regexFilter` become `url-filter` regexes in WebKit's
//!   subset (no `|` alternation, no `{n}`, `^` only first and `$` only last).
//! - `requestDomains` become one rule per domain (no alternation).
//! - `initiatorDomains` / `excludedInitiatorDomains` become `if-domain` / `unless-domain`
//!   (`*domain`: the domain and its subdomains). WebKit takes one or the other on a trigger, so a
//!   block with both, or with `excludedRequestDomains`, becomes an *exception group*: the block,
//!   then `ignore-previous-rules` for each exception. The groups go first of all, so their
//!   exceptions undo only other groups' blocks.
//! - Priority: WebKit has none, but a later `ignore-previous-rules` undoes earlier matches and a
//!   later block applies again; so rules go lowest priority first, and within a priority in
//!   DNR's order of precedence reversed (redirect, block, allowAllRequests, allow).
//! - `redirect` to one of the extension's resources (a neutered script, a blank image) can't be
//!   done: it blocks instead. A redirect that rewrites the URL (`transform`, `url`) is skipped.
//! - `upgradeScheme`, `modifyHeaders`, and conditions WebKit has no word for (`requestMethods`,
//!   `tabIds`, `responseHeaders`) are skipped.

use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// WebKit's limit on the rules in one compiled list.
pub const WEBKIT_MAX_RULES: usize = 150_000;

/// Element-hiding rules taken at most (one per selector), so a huge list stays cheap to compile.
pub const MAX_COSMETIC: usize = 25_000;

/// Why a rule was skipped. Kept as counts, by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Skip {
    /// `upgradeScheme`: WebKit can't change a URL.
    UpgradeScheme,
    /// `modifyHeaders`: WebKit can't change headers.
    ModifyHeaders,
    /// A redirect that rewrites the URL (`transform`, `url`, `regexSubstitution`).
    RedirectRewrite,
    /// `requestMethods` / `excludedRequestMethods`: WebKit matches every method.
    RequestMethods,
    /// `tabIds`, `responseHeaders` and other conditions WebKit has no word for.
    Condition,
    /// A `regexFilter` outside WebKit's regex subset.
    Regex,
    /// A `urlFilter` that can't be written as a WebKit regex (not ASCII, or anchored with
    /// `requestDomains` beside it).
    Pattern,
    /// Its domains weren't plain ASCII (WebKit wants them lowercase, punycode).
    Domains,
    /// An `allowAllRequests` for sub-frames only, or with domains beside a page pattern.
    AllowAllRequests,
    /// An action this doesn't know.
    Unknown,
}

impl Skip {
    pub fn label(self) -> &'static str {
        match self {
            Skip::UpgradeScheme => "upgradeScheme",
            Skip::ModifyHeaders => "modifyHeaders",
            Skip::RedirectRewrite => "URL-rewriting redirects",
            Skip::RequestMethods => "request methods",
            Skip::Condition => "other conditions",
            Skip::Regex => "regexes",
            Skip::Pattern => "patterns",
            Skip::Domains => "domains",
            Skip::AllowAllRequests => "allowAllRequests",
            Skip::Unknown => "unknown actions",
        }
    }
}

/// What a conversion did with the rules it was given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    /// DNR rules read.
    pub total: usize,
    /// DNR rules that became WebKit rules (redirects among them).
    pub active: usize,
    /// Redirects that block instead.
    pub redirects: usize,
    /// DNR rules skipped, by why.
    pub skipped: BTreeMap<Skip, usize>,
    /// WebKit rules made from the DNR rules (one DNR rule can make several).
    pub webkit_rules: usize,
    /// Element-hiding rules (one per selector, on its sites).
    pub cosmetic: usize,
}

impl Stats {
    pub fn skipped_total(&self) -> usize {
        self.skipped.values().sum()
    }
}

/// One DNR rule, as an extension's ruleset has it.
#[derive(Debug, Deserialize)]
pub struct DnrRule {
    #[serde(default)]
    pub id: i64,
    #[serde(default = "default_priority")]
    pub priority: i64,
    pub action: DnrAction,
    #[serde(default)]
    pub condition: DnrCondition,
}

fn default_priority() -> i64 {
    1
}

#[derive(Debug, Deserialize)]
pub struct DnrAction {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub redirect: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DnrCondition {
    pub url_filter: Option<String>,
    pub regex_filter: Option<String>,
    pub is_url_filter_case_sensitive: Option<bool>,
    pub initiator_domains: Option<Vec<String>>,
    pub excluded_initiator_domains: Option<Vec<String>>,
    /// The old names of `initiatorDomains` / `excludedInitiatorDomains`.
    pub domains: Option<Vec<String>>,
    pub excluded_domains: Option<Vec<String>>,
    pub request_domains: Option<Vec<String>>,
    pub excluded_request_domains: Option<Vec<String>>,
    pub resource_types: Option<Vec<String>>,
    pub excluded_resource_types: Option<Vec<String>>,
    pub request_methods: Option<Vec<String>>,
    pub excluded_request_methods: Option<Vec<String>>,
    pub domain_type: Option<String>,
    /// Everything else (`tabIds`, `responseHeaders`…): WebKit has nothing for them.
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// The rules made, in the order WebKit must see them, and what became of the source.
#[derive(Debug, Default)]
pub struct Converted {
    /// Network rules: exception groups, then by priority.
    pub network: Vec<Value>,
    /// Element hiding, after the network rules (so a page's own allow rule can't undo them).
    pub cosmetic: Vec<Value>,
    pub stats: Stats,
}

impl Converted {
    /// Every rule in order, then one that lets ads load on each of `allowed` sites; in lists
    /// of at most WebKit's limit. The allow rule goes at the end of every list, since
    /// `ignore-previous-rules` only reaches back within its own list.
    pub fn lists(rules: &[Value], allowed: &[String]) -> Vec<String> {
        let allow = allow_sites_rule(allowed);
        let room = WEBKIT_MAX_RULES - usize::from(allow.is_some());
        let mut out = vec![];
        for chunk in rules.chunks(room.max(1)) {
            let mut list: Vec<&Value> = chunk.iter().collect();
            list.extend(allow.as_ref());
            out.push(serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()));
        }
        if out.is_empty()
            && let Some(allow) = &allow
        {
            out.push(Value::Array(vec![allow.clone()]).to_string());
        }
        out
    }
}

/// The rule for "Allow ads on this site": nothing before it applies on those sites' pages.
pub fn allow_sites_rule(allowed: &[String]) -> Option<Value> {
    let domains: Vec<String> = allowed.iter().filter_map(|d| webkit_domain(d)).collect();
    (!domains.is_empty()).then(|| {
        json!({
            "trigger": { "url-filter": ".*", "if-domain": domains },
            "action": { "type": "ignore-previous-rules" },
        })
    })
}

/// What a DNR action becomes in WebKit, by DNR's precedence (higher wins at one priority).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Redirect,
    Block,
    AllowAll,
    Allow,
}

/// Every resource type DNR knows.
const DNR_TYPES: &[&str] = &[
    "main_frame",
    "sub_frame",
    "stylesheet",
    "script",
    "image",
    "font",
    "object",
    "xmlhttprequest",
    "ping",
    "csp_report",
    "media",
    "websocket",
    "webtransport",
    "webbundle",
    "other",
];

/// WebKit's name for a DNR resource type (frames are `document`, handled apart). `raw` is
/// WebKit's catch-all for fetches, sockets, beacons and the rest, in every WebKit that has
/// content blockers (`fetch`, `websocket`, `ping` came later).
fn webkit_type(dnr: &str) -> Option<&'static str> {
    Some(match dnr {
        "stylesheet" => "style-sheet",
        "script" => "script",
        "image" => "image",
        "font" => "font",
        "media" => "media",
        "object" | "xmlhttprequest" | "ping" | "csp_report" | "websocket" | "webtransport"
        | "webbundle" | "other" => "raw",
        _ => return None,
    })
}

/// A domain as WebKit's `if-domain` takes it: lowercase ASCII, `*` for its subdomains too.
pub fn webkit_domain(domain: &str) -> Option<String> {
    let d = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    let ok = !d.is_empty()
        && d.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-');
    ok.then(|| format!("*{d}"))
}

/// A literal for a WebKit regex.
pub fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if matches!(
            c,
            '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '\\'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The start of a URL up to where a host name may begin, at a label: `||`.
const HOST_START: &str = "^[^:]+://([^/]+\\.)?";
/// DNR's `^` separator: a character that's none of these.
const SEPARATOR: &str = "[^a-zA-Z0-9_.%-]";
/// A `^` at the end: a separator and anything after it, or the URL's end. (No alternation in
/// WebKit, so an optional group before `$`.)
const SEPARATOR_OR_END: &str = "([^a-zA-Z0-9_.%-].*)?$";

/// A DNR `urlFilter` as a WebKit regex, or `None` when it can't be.
pub fn url_filter_regex(filter: &str) -> Option<String> {
    if !filter.is_ascii() {
        return None;
    }
    let mut s = filter;
    let mut out = String::new();
    let host = s.starts_with("||");
    if host {
        s = &s[2..];
        out.push_str(HOST_START);
    } else if let Some(rest) = s.strip_prefix('|') {
        s = rest;
        out.push('^');
    }
    let end = s.ends_with('|');
    if end {
        s = &s[..s.len() - 1];
    }
    // Wildcards at either end say nothing (WebKit's filter isn't anchored unless asked).
    if out.is_empty() {
        s = s.trim_start_matches('*');
    }
    if !end {
        s = s.trim_end_matches('*');
    }
    // Still in the host (no `/` yet, after `||`): a `^` there is the host's end.
    let mut in_host = host;
    let n = s.len();
    for (i, c) in s.char_indices() {
        match c {
            '*' => out.push_str(".*"),
            '^' if i + 1 == n && !end => {
                out.push_str(if in_host { "[:/]" } else { SEPARATOR_OR_END });
            }
            '^' => out.push_str(SEPARATOR),
            '|' => return None,
            c => {
                if c == '/' || c == '?' {
                    in_host = false;
                }
                out.push_str(&regex_escape(c.encode_utf8(&mut [0; 4])));
            }
        }
    }
    if end {
        out.push('$');
    }
    if out.is_empty() || out == "^" {
        out = ".*".into();
    }
    Some(out)
}

/// A DNR `regexFilter` (RE2) in WebKit's subset, or `None` when it uses more: alternation,
/// counted repetition, lookaround, back-references, lazy quantifiers, word boundaries, or `^` /
/// `$` anywhere but the ends. `\d` and `\w` are spelled out; `(?:` is a plain group.
pub fn regex_filter_regex(re: &str) -> Option<String> {
    if !re.is_ascii() || re.is_empty() {
        return None;
    }
    let b = re.as_bytes();
    let mut out = String::with_capacity(re.len() + 16);
    let mut i = 0;
    let mut in_class = false;
    let mut depth = 0i32;
    // Whether what came last can take a quantifier (not a quantifier itself).
    let mut quantifiable = false;
    while i < b.len() {
        let c = b[i] as char;
        if c == '\\' {
            let next = *b.get(i + 1)? as char;
            let piece = match next {
                'd' if in_class => "0-9".to_string(),
                'w' if in_class => "a-zA-Z0-9_".to_string(),
                'd' => "[0-9]".to_string(),
                'w' => "[a-zA-Z0-9_]".to_string(),
                c if c.is_ascii_alphanumeric() => return None,
                c => format!("\\{c}"),
            };
            out.push_str(&piece);
            i += 2;
            quantifiable = true;
            continue;
        }
        if in_class {
            if c == ']' {
                in_class = false;
                quantifiable = true;
            }
            out.push(c);
            i += 1;
            continue;
        }
        match c {
            '[' => {
                in_class = true;
                out.push('[');
                // A `]` or `^]` right after the opening is a literal; copy through it.
                if b.get(i + 1) == Some(&b'^') {
                    out.push('^');
                    i += 1;
                }
                if b.get(i + 1) == Some(&b']') {
                    out.push_str("\\]");
                    i += 1;
                }
            }
            '|' | '{' | '}' => return None,
            '(' => {
                if b.get(i + 1) == Some(&b'?') {
                    if b.get(i + 2) == Some(&b':') {
                        i += 2;
                    } else {
                        return None;
                    }
                }
                depth += 1;
                out.push('(');
                quantifiable = false;
            }
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
                out.push(')');
                quantifiable = true;
            }
            '*' | '+' | '?' => {
                if !quantifiable {
                    return None; // A lazy or doubled quantifier, or one with nothing before it.
                }
                out.push(c);
                quantifiable = false;
            }
            '^' if i == 0 => out.push('^'),
            '$' if i + 1 == b.len() => out.push('$'),
            '^' | '$' => return None,
            c => {
                out.push(c);
                quantifiable = true;
            }
        }
        i += 1;
    }
    (!in_class && depth == 0).then_some(out)
}

/// The resource types a trigger covers, as WebKit splits them: the non-frame ones, and the frame
/// documents (`document` for both, or one with a load context).
#[derive(Debug, PartialEq, Eq)]
struct Types {
    /// WebKit types for the rest, deduplicated; empty when there are none.
    other: Vec<&'static str>,
    main_frame: bool,
    sub_frame: bool,
    /// Every type DNR has (WebKit then needs no `resource-type` at all).
    all: bool,
}

fn types(cond: &DnrCondition) -> Types {
    let set: BTreeSet<&str> = match (&cond.resource_types, &cond.excluded_resource_types) {
        (Some(only), _) => only.iter().map(String::as_str).collect(),
        (None, Some(not)) => DNR_TYPES
            .iter()
            .copied()
            .filter(|t| !not.iter().any(|n| n == t))
            .collect(),
        // DNR's default: everything but the page itself.
        (None, None) => DNR_TYPES
            .iter()
            .copied()
            .filter(|t| *t != "main_frame")
            .collect(),
    };
    let mut other: Vec<&'static str> = set.iter().filter_map(|t| webkit_type(t)).collect();
    other.sort_unstable();
    other.dedup();
    Types {
        other,
        main_frame: set.contains("main_frame"),
        sub_frame: set.contains("sub_frame"),
        all: DNR_TYPES.iter().all(|t| set.contains(t)),
    }
}

/// The triggers' type parts for `types`: `resource-type` (and `load-context` when only one kind
/// of frame), one per trigger needed.
fn type_parts(t: &Types) -> Vec<Map<String, Value>> {
    let mut parts = vec![];
    if t.all {
        parts.push(Map::new());
        return parts;
    }
    let both_frames = t.main_frame && t.sub_frame;
    let mut rest: Vec<&str> = t.other.clone();
    if both_frames {
        rest.push("document");
    }
    if !rest.is_empty() {
        let mut m = Map::new();
        m.insert("resource-type".into(), json!(rest));
        parts.push(m);
    }
    if t.main_frame != t.sub_frame {
        let mut m = Map::new();
        m.insert("resource-type".into(), json!(["document"]));
        let context = if t.main_frame {
            "top-frame"
        } else {
            "child-frame"
        };
        m.insert("load-context".into(), json!([context]));
        parts.push(m);
    }
    parts
}

/// A list of domains for WebKit, or `Err` when some aren't plain ASCII and so the rule can't be
/// trusted (an include list loses entries: it would match less; an exclude list: more).
fn domain_list(list: &Option<Vec<String>>) -> Result<Vec<String>, Skip> {
    let Some(list) = list else {
        return Ok(vec![]);
    };
    let mut out: Vec<String> = list
        .iter()
        .map(|d| webkit_domain(d).ok_or(Skip::Domains))
        .collect::<Result<_, _>>()?;
    out.sort();
    out.dedup();
    Ok(out)
}

/// A request domain's URL prefix: its scheme, any subdomain, the domain, then a port or path.
fn request_domain_regex(domain: &str) -> Option<String> {
    let d = webkit_domain(domain)?;
    Some(format!("{HOST_START}{}[:/]", regex_escape(&d[1..])))
}

/// One DNR rule's WebKit rules (in order), or why it was skipped.
struct Made {
    kind: Kind,
    priority: i64,
    /// Whether it's an exception group (goes first of all).
    group: bool,
    rules: Vec<Value>,
}

fn convert_rule(rule: &DnrRule) -> Result<Made, Skip> {
    let c = &rule.condition;
    let kind = match rule.action.kind.as_str() {
        "block" => Kind::Block,
        "allow" => Kind::Allow,
        "allowAllRequests" => Kind::AllowAll,
        "redirect" => {
            let to_resource = rule
                .action
                .redirect
                .as_ref()
                .is_some_and(|r| r.get("extensionPath").is_some());
            if !to_resource {
                return Err(Skip::RedirectRewrite);
            }
            Kind::Redirect
        }
        "upgradeScheme" => return Err(Skip::UpgradeScheme),
        "modifyHeaders" => return Err(Skip::ModifyHeaders),
        _ => return Err(Skip::Unknown),
    };
    if c.request_methods.is_some() || c.excluded_request_methods.is_some() {
        return Err(Skip::RequestMethods);
    }
    if !c.other.is_empty() {
        return Err(Skip::Condition);
    }

    // The URL part.
    let pattern = match (&c.url_filter, &c.regex_filter) {
        (Some(f), _) => Some(url_filter_regex(f).ok_or(Skip::Pattern)?),
        (None, Some(r)) => Some(regex_filter_regex(r).ok_or(Skip::Regex)?),
        (None, None) => None,
    };
    let anchored = pattern.as_deref().is_some_and(|p| p.starts_with('^'));
    let request_domains = c.request_domains.clone().unwrap_or_default();
    let url_filters: Vec<String> = if request_domains.is_empty() {
        vec![pattern.clone().unwrap_or_else(|| ".*".into())]
    } else {
        let mut out = vec![];
        for d in &request_domains {
            let host = request_domain_regex(d).ok_or(Skip::Domains)?;
            out.push(match &pattern {
                None => host,
                // A regex anchored to the URL's start says its own host: it stands alone.
                Some(p) if anchored && c.regex_filter.is_some() && c.url_filter.is_none() => {
                    p.clone()
                }
                Some(_) if anchored => return Err(Skip::Pattern),
                Some(p) => format!("{host}.*{p}"),
            });
        }
        out.sort();
        out.dedup();
        out
    };

    let include = {
        let mut v = domain_list(&c.initiator_domains)?;
        v.extend(domain_list(&c.domains)?);
        v
    };
    let exclude = {
        let mut v = domain_list(&c.excluded_initiator_domains)?;
        v.extend(domain_list(&c.excluded_domains)?);
        v
    };
    let excluded_requests: Vec<String> = c
        .excluded_request_domains
        .iter()
        .flatten()
        .map(|d| request_domain_regex(d).ok_or(Skip::Domains))
        .collect::<Result<_, _>>()?;

    let mut base = Map::new();
    if c.is_url_filter_case_sensitive == Some(true) {
        base.insert("url-filter-is-case-sensitive".into(), json!(true));
    }
    match c.domain_type.as_deref() {
        Some("firstParty") => {
            base.insert("load-type".into(), json!(["first-party"]));
        }
        Some("thirdParty") => {
            base.insert("load-type".into(), json!(["third-party"]));
        }
        _ => {}
    }

    if kind == Kind::AllowAll {
        return allow_all(rule, &pattern, &include, &exclude, base);
    }

    // A block with exceptions WebKit can't put on its trigger: the group.
    let blocking = matches!(kind, Kind::Block | Kind::Redirect);
    let group =
        blocking && ((!include.is_empty() && !exclude.is_empty()) || !excluded_requests.is_empty());
    if !include.is_empty() {
        base.insert("if-domain".into(), json!(include));
    } else if !exclude.is_empty() {
        base.insert("unless-domain".into(), json!(exclude));
    }
    // (An allow's exceptions are dropped: it lets a little more through, never blocks more.)

    let action = if blocking {
        json!({ "type": "block" })
    } else {
        json!({ "type": "ignore-previous-rules" })
    };
    let parts = type_parts(&types(c));
    let mut rules = vec![];
    for filter in &url_filters {
        for part in &parts {
            let mut trigger = base.clone();
            trigger.insert("url-filter".into(), json!(filter));
            trigger.extend(part.clone());
            rules.push(json!({ "trigger": trigger, "action": action }));
        }
    }
    if group {
        let undo = json!({ "type": "ignore-previous-rules" });
        // The excluded initiators, where the block's own domains are on its trigger.
        if !include.is_empty() && !exclude.is_empty() {
            for filter in &url_filters {
                for part in &parts {
                    let mut trigger = base.clone();
                    trigger.insert("url-filter".into(), json!(filter));
                    trigger.insert("if-domain".into(), json!(exclude));
                    trigger.extend(part.clone());
                    rules.push(json!({ "trigger": trigger, "action": undo }));
                }
            }
        }
        for host in &excluded_requests {
            for part in &parts {
                let mut trigger = base.clone();
                trigger.insert("url-filter".into(), json!(host));
                trigger.extend(part.clone());
                rules.push(json!({ "trigger": trigger, "action": undo }));
            }
        }
    }
    Ok(Made {
        kind,
        priority: rule.priority,
        group,
        rules,
    })
}

/// `allowAllRequests`: everything a page (main frame) loads is let through. A page named by its
/// host alone becomes `if-domain`; another pattern, `if-top-url`. Sub-frames: no WebKit word.
fn allow_all(
    rule: &DnrRule,
    pattern: &Option<String>,
    include: &[String],
    exclude: &[String],
    mut base: Map<String, Value>,
) -> Result<Made, Skip> {
    let t = types(&rule.condition);
    if !t.main_frame {
        return Err(Skip::AllowAllRequests);
    }
    let host = rule
        .condition
        .url_filter
        .as_deref()
        .and_then(|f| f.strip_prefix("||"))
        .map(|h| h.trim_end_matches('^'))
        .filter(|h| !h.contains(['/', '*', '^', '|', '?']))
        .and_then(webkit_domain);
    base.remove("load-type");
    base.insert("url-filter".into(), json!(".*"));
    match (host, pattern) {
        (Some(host), _) if include.is_empty() && exclude.is_empty() => {
            base.insert("if-domain".into(), json!([host]));
        }
        (None, Some(p)) if include.is_empty() && exclude.is_empty() => {
            base.insert("if-top-url".into(), json!([p]));
        }
        (None, None) if !include.is_empty() => {
            base.insert("if-domain".into(), json!(include));
        }
        _ => return Err(Skip::AllowAllRequests),
    }
    Ok(Made {
        kind: Kind::AllowAll,
        priority: rule.priority,
        group: false,
        rules: vec![json!({ "trigger": base, "action": { "type": "ignore-previous-rules" } })],
    })
}

/// Convert `rules` (every ruleset's, in order) and `cosmetic` (element hiding, from
/// `cosmetic_rules`).
pub fn convert(rules: &[DnrRule], cosmetic: Vec<Value>) -> Converted {
    let mut stats = Stats {
        total: rules.len(),
        ..Default::default()
    };
    let mut groups = vec![];
    let mut made = vec![];
    for rule in rules {
        match convert_rule(rule) {
            Ok(m) => {
                stats.active += 1;
                if m.kind == Kind::Redirect {
                    stats.redirects += 1;
                }
                if m.group {
                    groups.push(m);
                } else {
                    made.push(m);
                }
            }
            Err(why) => *stats.skipped.entry(why).or_default() += 1,
        }
    }
    // Lowest priority first; within one, the weakest action first: what's later wins.
    made.sort_by_key(|m| (m.priority, m.kind));
    let network: Vec<Value> = groups
        .into_iter()
        .chain(made)
        .flat_map(|m| m.rules)
        .collect();
    stats.webkit_rules = network.len();
    stats.cosmetic = cosmetic.len();
    Converted {
        network,
        cosmetic,
        stats,
    }
}

/// Pseudo-classes WebKit's CSS knows that hiding rules may use; anything else (uBlock's own
/// procedural ones, `:has-text()`, `:upward()`…) isn't CSS, and the rule is left out.
const PSEUDO_CLASSES: &[&str] = &[
    "not",
    "has",
    "is",
    "where",
    "nth-child",
    "nth-last-child",
    "nth-of-type",
    "nth-last-of-type",
    "first-child",
    "last-child",
    "first-of-type",
    "last-of-type",
    "only-child",
    "only-of-type",
    "empty",
    "root",
    "checked",
    "disabled",
    "enabled",
    "link",
    "visited",
];

/// Whether `selector` is plain CSS this takes: printable ASCII, balanced, and only the
/// pseudo-classes above (no pseudo-elements).
pub fn plain_selector(selector: &str) -> bool {
    if selector.is_empty()
        || selector.len() > 1000
        || !selector.bytes().all(|b| (0x20..0x7f).contains(&b))
        || selector.contains(['{', '}', '\\'])
        || selector.starts_with(['+', '^'])
    {
        return false;
    }
    let b = selector.as_bytes();
    let (mut parens, mut brackets) = (0i32, 0i32);
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' | b'\'' => quote = Some(c),
            b'[' => brackets += 1,
            b']' => brackets -= 1,
            b'(' => parens += 1,
            b')' => parens -= 1,
            b':' if brackets == 0 => {
                if b.get(i + 1) == Some(&b':') {
                    return false;
                }
                let name: String = selector[i + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                if !PSEUDO_CLASSES.contains(&name.as_str()) {
                    return false;
                }
            }
            _ => {}
        }
        if parens < 0 || brackets < 0 {
            return false;
        }
        i += 1;
    }
    parens == 0 && brackets == 0 && quote.is_none()
}

/// Element hiding from filter lists' text: each site-specific `site.com,other.org##selector`
/// becomes a `css-display-none` rule on those sites (the selector's sites gathered from every
/// line). Generic hiding (no sites), exceptions to generic hiding, procedural and scriptlet
/// filters are left out; a site's own `#@#` exception removes the site from that selector.
/// At most `MAX_COSMETIC` rules.
pub fn cosmetic_rules<'a>(lists: impl IntoIterator<Item = &'a str>) -> Vec<Value> {
    let mut by_selector: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut exceptions: BTreeSet<(String, String)> = BTreeSet::new();
    for text in lists {
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('!') || line.starts_with('[') {
                continue;
            }
            let (sites, selector, exception) = if let Some((s, sel)) = line.split_once("#@#") {
                (s, sel, true)
            } else if let Some((s, sel)) = line.split_once("##") {
                (s, sel, false)
            } else {
                continue;
            };
            // `#?#`, `#$#`, `#%#` (procedural, style, script) split as `#?` sites: none plain.
            if sites.is_empty() || !plain_selector(selector) {
                continue;
            }
            let sites: Vec<String> = sites
                .split(',')
                .filter(|s| !s.starts_with('~') && !s.contains(['*', '/', '#', '$']))
                .filter_map(webkit_domain)
                .collect();
            for site in sites {
                if exception {
                    exceptions.insert((site, selector.to_string()));
                } else {
                    by_selector
                        .entry(selector.to_string())
                        .or_default()
                        .insert(site);
                }
            }
        }
    }
    let mut out = vec![];
    for (selector, sites) in by_selector {
        let sites: Vec<String> = sites
            .into_iter()
            .filter(|s| !exceptions.contains(&(s.clone(), selector.clone())))
            .collect();
        if sites.is_empty() {
            continue;
        }
        if out.len() == MAX_COSMETIC {
            break;
        }
        out.push(json!({
            "trigger": { "url-filter": ".*", "if-domain": sites },
            "action": { "type": "css-display-none", "selector": selector },
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(v: Value) -> DnrRule {
        serde_json::from_value(v).unwrap()
    }

    fn one(v: Value) -> Vec<Value> {
        let c = convert(&[rule(v)], vec![]);
        assert_eq!(c.stats.skipped_total(), 0, "{:?}", c.stats);
        c.network
    }

    fn skip(v: Value) -> Skip {
        let c = convert(&[rule(v)], vec![]);
        assert_eq!(c.stats.active, 0);
        *c.stats.skipped.keys().next().unwrap()
    }

    #[test]
    fn url_filters_become_webkit_regexes() {
        assert_eq!(
            url_filter_regex("||ads.example.com^").unwrap(),
            "^[^:]+://([^/]+\\.)?ads\\.example\\.com[:/]"
        );
        assert_eq!(
            url_filter_regex("||example.com/ad/*.js").unwrap(),
            "^[^:]+://([^/]+\\.)?example\\.com/ad/.*\\.js"
        );
        // `^` in a path: a separator, or the end at the end.
        assert_eq!(
            url_filter_regex("/ads.js^").unwrap(),
            "/ads\\.js([^a-zA-Z0-9_.%-].*)?$"
        );
        assert_eq!(
            url_filter_regex("||x.com^*/pop^").unwrap(),
            "^[^:]+://([^/]+\\.)?x\\.com[^a-zA-Z0-9_.%-].*/pop([^a-zA-Z0-9_.%-].*)?$"
        );
        // `|` anchors; wildcards at the ends drop.
        assert_eq!(
            url_filter_regex("|https://a.b/").unwrap(),
            "^https://a\\.b/"
        );
        assert_eq!(url_filter_regex("/track.gif|").unwrap(), "/track\\.gif$");
        assert_eq!(url_filter_regex("*/banner/*").unwrap(), "/banner/");
        assert_eq!(
            url_filter_regex("?ad=1&x=(y)").unwrap(),
            "\\?ad=1&x=\\(y\\)"
        );
        assert_eq!(url_filter_regex("*").unwrap(), ".*");
        assert_eq!(url_filter_regex("").unwrap(), ".*");
        // Not ASCII, or a `|` mid-pattern: can't.
        assert_eq!(url_filter_regex("||пример.рф^"), None);
        assert_eq!(url_filter_regex("a|b"), None);
    }

    #[test]
    fn regex_filters_keep_to_webkits_subset() {
        assert_eq!(
            regex_filter_regex("^https?:\\/\\/[a-z]+-x\\.com\\/").unwrap(),
            "^https?:\\/\\/[a-z]+-x\\.com\\/"
        );
        assert_eq!(
            regex_filter_regex("\\/js\\/[a-z0-9]+\\.js$").unwrap(),
            "\\/js\\/[a-z0-9]+\\.js$"
        );
        assert_eq!(
            regex_filter_regex("^/\\d+\\w/").unwrap(),
            "^/[0-9]+[a-zA-Z0-9_]/"
        );
        assert_eq!(regex_filter_regex("[\\d.]+").unwrap(), "[0-9.]+");
        assert_eq!(regex_filter_regex("(?:ab)+c").unwrap(), "(ab)+c");
        for no in [
            "a|b", "a{2}", "(?=x)", "a*?", "a++", "\\bad", "\\s", "x^y", "x$y", "(a", "a)", "[ab",
            "é",
        ] {
            assert_eq!(regex_filter_regex(no), None, "{no}");
        }
    }

    #[test]
    fn a_block_with_no_types_skips_the_page_itself() {
        let r = one(json!({"id": 1, "priority": 10, "action": {"type": "block"},
            "condition": {"urlFilter": "||ads.com^"}}));
        // Everything but the main frame: the rest, then sub-frame documents apart.
        assert_eq!(r.len(), 2);
        let types = r[0]["trigger"]["resource-type"].as_array().unwrap();
        assert!(!types.iter().any(|t| t == "document"));
        assert!(types.iter().any(|t| t == "script") && types.iter().any(|t| t == "raw"));
        assert_eq!(r[1]["trigger"]["resource-type"], json!(["document"]));
        assert_eq!(r[1]["trigger"]["load-context"], json!(["child-frame"]));
        assert_eq!(r[0]["action"]["type"], "block");
    }

    #[test]
    fn resource_types_map_and_frames_split() {
        let r = one(json!({"action": {"type": "block"}, "condition": {
            "urlFilter": "/x.", "resourceTypes": ["script", "xmlhttprequest", "ping", "image"]}}));
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0]["trigger"]["resource-type"],
            json!(["image", "raw", "script"])
        );
        assert!(r[0]["trigger"].get("load-context").is_none());

        let r = one(json!({"action": {"type": "block"}, "condition": {
            "urlFilter": "/x.", "resourceTypes": ["main_frame"]}}));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["trigger"]["resource-type"], json!(["document"]));
        assert_eq!(r[0]["trigger"]["load-context"], json!(["top-frame"]));

        let r = one(json!({"action": {"type": "block"}, "condition": {
            "urlFilter": "/x.", "resourceTypes": ["main_frame", "sub_frame", "font"]}}));
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0]["trigger"]["resource-type"],
            json!(["font", "document"])
        );

        // Excluded types: everything else, the page included.
        let r = one(json!({"action": {"type": "block"}, "condition": {
            "urlFilter": "/x.", "excludedResourceTypes": ["main_frame"]}}));
        assert_eq!(r.len(), 2);
        let r = one(json!({"action": {"type": "block"}, "condition": {
            "urlFilter": "/x.", "excludedResourceTypes": ["script"]}}));
        assert_eq!(r.len(), 1);
        let types = r[0]["trigger"]["resource-type"].as_array().unwrap();
        assert!(types.iter().any(|t| t == "document") && !types.iter().any(|t| t == "script"));
    }

    #[test]
    fn domains_become_if_and_unless_domain() {
        let r = one(json!({"action": {"type": "block"}, "condition": {
            "urlFilter": "/pop.js", "resourceTypes": ["script"],
            "initiatorDomains": ["B.com", "a.org."], "domainType": "thirdParty",
            "isUrlFilterCaseSensitive": true}}));
        let t = &r[0]["trigger"];
        assert_eq!(t["if-domain"], json!(["*a.org", "*b.com"]));
        assert!(t.get("unless-domain").is_none());
        assert_eq!(t["load-type"], json!(["third-party"]));
        assert_eq!(t["url-filter-is-case-sensitive"], json!(true));

        let r = one(json!({"action": {"type": "block"}, "condition": {
            "urlFilter": "/pop.js", "resourceTypes": ["script"],
            "excludedInitiatorDomains": ["a.org"], "domainType": "firstParty"}}));
        assert_eq!(r[0]["trigger"]["unless-domain"], json!(["*a.org"]));
        assert_eq!(r[0]["trigger"]["load-type"], json!(["first-party"]));

        // The old names too.
        let r = one(json!({"action": {"type": "block"}, "condition": {
            "urlFilter": "/pop.js", "resourceTypes": ["script"], "domains": ["c.net"]}}));
        assert_eq!(r[0]["trigger"]["if-domain"], json!(["*c.net"]));

        assert_eq!(
            skip(json!({"action": {"type": "block"}, "condition": {
                "urlFilter": "/a", "initiatorDomains": ["пример.рф"]}})),
            Skip::Domains
        );
    }

    #[test]
    fn request_domains_make_a_rule_each() {
        let r = one(json!({"action": {"type": "block"}, "condition": {
            "requestDomains": ["ads.net", "track.io"], "resourceTypes": ["script"]}}));
        assert_eq!(r.len(), 2);
        assert_eq!(
            r[0]["trigger"]["url-filter"],
            "^[^:]+://([^/]+\\.)?ads\\.net[:/]"
        );
        // With a path pattern: the domain, then the pattern later in the URL.
        let r = one(json!({"action": {"type": "block"}, "condition": {
            "requestDomains": ["cdn.com"], "urlFilter": "/ad/", "resourceTypes": ["image"]}}));
        assert_eq!(
            r[0]["trigger"]["url-filter"],
            "^[^:]+://([^/]+\\.)?cdn\\.com[:/].*/ad/"
        );
        // An anchored pattern beside them: can't be both.
        assert_eq!(
            skip(json!({"action": {"type": "block"}, "condition": {
                "requestDomains": ["cdn.com"], "urlFilter": "||x.com^"}})),
            Skip::Pattern
        );
    }

    #[test]
    fn exceptions_webkit_cant_hold_become_groups_first() {
        let rules = [
            rule(json!({"id": 1, "priority": 10, "action": {"type": "block"},
                "condition": {"urlFilter": "/a.js", "resourceTypes": ["script"]}})),
            rule(
                json!({"id": 2, "priority": 10, "action": {"type": "block"}, "condition": {
                "initiatorDomains": ["fp.com"], "excludedInitiatorDomains": ["dev.fp.com"],
                "excludedRequestDomains": ["github.com"], "resourceTypes": ["script"]}}),
            ),
        ];
        let c = convert(&rules, vec![]);
        assert_eq!(c.stats.active, 2);
        let n = &c.network;
        assert_eq!(n.len(), 4);
        // The group: its block, its excluded initiator, its excluded request domain; then the rest.
        assert_eq!(n[0]["action"]["type"], "block");
        assert_eq!(n[0]["trigger"]["if-domain"], json!(["*fp.com"]));
        assert_eq!(n[1]["action"]["type"], "ignore-previous-rules");
        assert_eq!(n[1]["trigger"]["if-domain"], json!(["*dev.fp.com"]));
        assert_eq!(n[2]["action"]["type"], "ignore-previous-rules");
        assert_eq!(
            n[2]["trigger"]["url-filter"],
            "^[^:]+://([^/]+\\.)?github\\.com[:/]"
        );
        assert_eq!(n[3]["trigger"]["url-filter"], "/a\\.js");
        // Never both on one trigger.
        for r in n {
            let t = &r["trigger"];
            assert!(!(t.get("if-domain").is_some() && t.get("unless-domain").is_some()));
        }
    }

    #[test]
    fn allows_follow_blocks_by_priority() {
        let rules = [
            rule(
                json!({"id": 1, "priority": 41, "action": {"type": "redirect",
                "redirect": {"extensionPath": "/web_accessible_resources/noop.js"}},
                "condition": {"urlFilter": "||ima.com/sdk.js", "resourceTypes": ["script"]}}),
            ),
            rule(json!({"id": 2, "priority": 30, "action": {"type": "allow"},
                "condition": {"urlFilter": "||ima.com/sdk.js", "initiatorDomains": ["tv.com"],
                    "excludedInitiatorDomains": ["x.tv.com"]}})),
            rule(json!({"id": 3, "priority": 10, "action": {"type": "block"},
                "condition": {"urlFilter": "||ima.com^", "resourceTypes": ["script"]}})),
            rule(json!({"id": 4, "priority": 10, "action": {"type": "allow"},
                "condition": {"urlFilter": "||ima.com/ok.js", "resourceTypes": ["script"]}})),
            rule(
                json!({"id": 5, "priority": 10, "action": {"type": "redirect",
                "redirect": {"extensionPath": "/1x1.gif"}},
                "condition": {"urlFilter": "/pixel.", "resourceTypes": ["image"]}}),
            ),
        ];
        let c = convert(&rules, vec![]);
        assert_eq!(c.stats.active, 5);
        assert_eq!(c.stats.redirects, 2);
        let order: Vec<(String, String)> = c
            .network
            .iter()
            .map(|r| {
                (
                    r["action"]["type"].as_str().unwrap().to_string(),
                    r["trigger"]["url-filter"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let at = |filter: &str| order.iter().position(|(_, f)| f.contains(filter)).unwrap();
        // Priority 10: the redirect, the block, then the allow; 30: the allow; 41: the redirect.
        assert!(at("pixel") < at("ima\\.com[:/]"));
        assert!(at("ima\\.com[:/]") < at("ok\\.js"));
        assert!(
            at("ok\\.js")
                < order
                    .iter()
                    .rposition(|(a, _)| a == "ignore-previous-rules")
                    .unwrap()
        );
        assert_eq!(order.last().unwrap().0, "block");
        assert!(order.last().unwrap().1.contains("sdk\\.js"));
        // An allow's excluded domains are dropped, never a group.
        let allow = c
            .network
            .iter()
            .find(|r| r["trigger"].get("if-domain") == Some(&json!(["*tv.com"])))
            .unwrap();
        assert_eq!(allow["action"]["type"], "ignore-previous-rules");
        assert!(allow["trigger"].get("unless-domain").is_none());
    }

    #[test]
    fn allow_all_requests_lets_a_page_through() {
        let r = one(
            json!({"priority": 30, "action": {"type": "allowAllRequests"},
            "condition": {"urlFilter": "||bank.com^", "resourceTypes": ["main_frame"]}}),
        );
        assert_eq!(r[0]["trigger"]["if-domain"], json!(["*bank.com"]));
        assert_eq!(r[0]["trigger"]["url-filter"], ".*");
        assert_eq!(r[0]["action"]["type"], "ignore-previous-rules");
        let r = one(
            json!({"priority": 30, "action": {"type": "allowAllRequests"},
            "condition": {"urlFilter": "|https://a.com/app", "resourceTypes": ["main_frame"]}}),
        );
        assert_eq!(
            r[0]["trigger"]["if-top-url"],
            json!(["^https://a\\.com/app"])
        );
        assert_eq!(
            skip(json!({"action": {"type": "allowAllRequests"},
                "condition": {"urlFilter": "||a.com^", "resourceTypes": ["sub_frame"]}})),
            Skip::AllowAllRequests
        );
    }

    #[test]
    fn what_webkit_cant_do_is_skipped_and_counted() {
        assert_eq!(
            skip(
                json!({"action": {"type": "redirect", "redirect": {"transform":
                {"queryTransform": {"removeParams": ["gclid"]}}}},
                "condition": {"urlFilter": "^gclid="}})
            ),
            Skip::RedirectRewrite
        );
        assert_eq!(
            skip(json!({"action": {"type": "upgradeScheme"}, "condition": {}})),
            Skip::UpgradeScheme
        );
        assert_eq!(
            skip(
                json!({"action": {"type": "modifyHeaders", "requestHeaders": []},
                "condition": {"urlFilter": "x"}})
            ),
            Skip::ModifyHeaders
        );
        assert_eq!(
            skip(json!({"action": {"type": "block"},
                "condition": {"urlFilter": "/x", "requestMethods": ["post"]}})),
            Skip::RequestMethods
        );
        assert_eq!(
            skip(json!({"action": {"type": "block"},
                "condition": {"urlFilter": "/x", "tabIds": [1]}})),
            Skip::Condition
        );
        assert_eq!(
            skip(json!({"action": {"type": "block"}, "condition": {"regexFilter": "a{3}"}})),
            Skip::Regex
        );
        assert_eq!(
            skip(json!({"action": {"type": "frobnicate"}, "condition": {}})),
            Skip::Unknown
        );
        let c = convert(
            &[
                rule(json!({"action": {"type": "upgradeScheme"}, "condition": {}})),
                rule(json!({"action": {"type": "upgradeScheme"}, "condition": {}})),
                rule(json!({"action": {"type": "block"}, "condition": {"urlFilter": "/ad/"}})),
            ],
            vec![],
        );
        assert_eq!(
            (c.stats.total, c.stats.active, c.stats.skipped_total()),
            (3, 1, 2)
        );
        assert_eq!(c.stats.skipped[&Skip::UpgradeScheme], 2);
    }

    #[test]
    fn lists_split_at_webkits_limit_with_the_sites_allowed_in_each() {
        let rules: Vec<Value> = (0..WEBKIT_MAX_RULES + 10)
            .map(|i| json!({"trigger": {"url-filter": format!("/{i}")}, "action": {"type": "block"}}))
            .collect();
        let lists = Converted::lists(&rules, &["News.com".into(), "bad domain".into()]);
        assert_eq!(lists.len(), 2);
        for list in &lists {
            let v: Vec<Value> = serde_json::from_str(list).unwrap();
            assert!(v.len() <= WEBKIT_MAX_RULES);
            let last = v.last().unwrap();
            assert_eq!(last["action"]["type"], "ignore-previous-rules");
            assert_eq!(last["trigger"]["if-domain"], json!(["*news.com"]));
        }
        assert_eq!(Converted::lists(&rules[..3], &[]).len(), 1);
        assert!(Converted::lists(&[], &[]).is_empty());
        assert_eq!(Converted::lists(&[], &["a.com".into()]).len(), 1);
    }

    #[test]
    fn element_hiding_takes_plain_site_specific_selectors() {
        let list = "! comment\n\
            ##.generic-ad\n\
            news.com,Blog.org##.sidebar-ad\n\
            other.net##.sidebar-ad\n\
            news.com##div[id^=\"ad-\"] > .x:not(.y)\n\
            news.com##.promo:has-text(Sponsored)\n\
            news.com#?#.thing:-abp-has(.ad)\n\
            news.com##+js(set, x, 1)\n\
            news.com##^script:has-text(ad)\n\
            news.com##.a::before\n\
            ~skip.com,keep.com##.banner\n\
            example.*##.wild\n\
            blog.org#@#.sidebar-ad\n";
        let rules = cosmetic_rules([list]);
        let by: BTreeMap<String, Value> = rules
            .iter()
            .map(|r| {
                (
                    r["action"]["selector"].as_str().unwrap().to_string(),
                    r["trigger"]["if-domain"].clone(),
                )
            })
            .collect();
        assert_eq!(by.len(), 3, "{by:?}");
        assert_eq!(by[".sidebar-ad"], json!(["*news.com", "*other.net"]));
        assert_eq!(by["div[id^=\"ad-\"] > .x:not(.y)"], json!(["*news.com"]));
        assert_eq!(by[".banner"], json!(["*keep.com"]));
        assert!(
            rules
                .iter()
                .all(|r| r["action"]["type"] == "css-display-none")
        );
    }

    #[test]
    fn plain_selectors_only() {
        assert!(plain_selector(".ad"));
        assert!(plain_selector("a[href*=\"click:here\"]"));
        assert!(plain_selector("div:nth-child(2) > span:has(> img)"));
        assert!(!plain_selector(".a:upward(2)"));
        assert!(!plain_selector(".a:style(display: none)"));
        assert!(!plain_selector(".a::after"));
        assert!(!plain_selector(".a)"));
        assert!(!plain_selector("[x=\"a]"));
        assert!(!plain_selector("é"));
        assert!(!plain_selector(""));
    }
}
