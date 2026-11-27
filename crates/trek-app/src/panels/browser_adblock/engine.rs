//! Trek's ad blocker's engine: uBlock Origin's lists (`lists`) read by Brave's `adblock` crate
//! (MPL-2.0), the engine Brave's browsers block with. It gives two things:
//!
//! - **Network filters as WebKit content rules** (`adblock`'s `content-blocking`, as Brave's iOS
//!   browser uses it): WebKit itself blocks the requests, in its own process, with no per-request
//!   cost in Trek. A list WebKit compiles takes at most 150,000 rules, so the rules are split into
//!   lists of up to [`MAX_RULES`], each carrying every exception rule after its blocks (an
//!   exception, `ignore-previous-rules`, only applies within its own list).
//! - **The engine itself**, kept in memory, for what happens in a page: the site's element hiding,
//!   the lists' generic hiding (asked by the page's classes and ids), procedural filters, and the
//!   scriptlets (`+js(…)`, uBlock Origin's own, `scriptlets`), per page as it loads (`page`).

use super::lists::{self, List};
use adblock::Engine;
use adblock::content_blocking::{CbRule, CbType};
use adblock::lists::{FilterFormat, FilterSet, ParseOptions, RuleTypes};
use std::path::Path;
use std::time::{Duration, Instant};

/// Rules in one WebKit list, at most (WebKit refuses a list over 150,000).
pub const MAX_RULES: usize = 140_000;

/// A list's text, as read for the engine.
pub struct Source {
    pub list: Option<&'static List>,
    pub text: String,
}

/// The lists in `base` that have been fetched, with what doesn't apply here taken out.
pub fn sources(base: &Path) -> Vec<Source> {
    lists::DEFAULTS
        .iter()
        .filter_map(|list| {
            let text = std::fs::read_to_string(lists::path(base, list)).ok()?;
            Some(Source {
                list: Some(list),
                text: lists::preprocess(&text),
            })
        })
        .collect()
}

fn options(source: &Source, rule_types: RuleTypes) -> ParseOptions {
    let (hosts, trusted) = source.list.map_or((false, true), |l| (l.hosts, l.trusted));
    ParseOptions {
        format: if hosts {
            FilterFormat::Hosts
        } else {
            FilterFormat::Standard
        },
        rule_types,
        // uBlock Origin's own lists and the user's may use its trusted scriptlets.
        permissions: if trusted {
            super::scriptlets::TRUSTED
        } else {
            Default::default()
        },
    }
}

/// What a build came to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub lists: usize,
    /// Network filters read, and how many became WebKit rules.
    pub network: usize,
    pub converted: usize,
    /// WebKit rules, in how many lists.
    pub rules: usize,
    pub webkit_lists: usize,
    /// Reading the lists into WebKit rules, building the page engine, and WebKit's compile (or
    /// finding the compiled lists in its store: `compiled` says how many it compiled).
    pub took: Duration,
    pub engine_took: Duration,
    pub webkit_took: Duration,
    pub compiled: usize,
}

/// Network filters of `sources` (and the user's own `user` lines) as WebKit content-rule lists
/// (JSON), each within [`MAX_RULES`].
/// A key for what `sources` and `user` hold, the same from one run to the next: WebKit's
/// compiled lists are found again by it (the rules' JSON itself isn't byte for byte the same each
/// time: the converter's sets have no order).
pub fn key(sources: &[Source], user: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for s in sources {
        eat(s.list.map_or("user", |l| l.id).as_bytes());
        eat(s.text.as_bytes());
    }
    // Only the user's network filters make WebKit rules: an element they picked to hide doesn't.
    for line in user.lines().filter(|l| is_network_line(l)) {
        eat(line.as_bytes());
    }
    // A newer converter makes other rules from the same lists.
    eat(env!("CARGO_PKG_VERSION").as_bytes());
    hash
}

pub fn content_rules(sources: &[Source], user: &str) -> (Vec<String>, Stats) {
    let started = Instant::now();
    let mut set = FilterSet::new(true);
    let mut network = 0;
    for s in sources {
        let record = set.add_filter_list(s.text.clone(), options(s, RuleTypes::NetworkOnly));
        let _ = record;
        network += s.text.lines().filter(|l| is_network_line(l)).count();
    }
    let user_source = Source {
        list: None,
        text: user.to_string(),
    };
    set.add_filter_list(
        user.to_string(),
        options(&user_source, RuleTypes::NetworkOnly),
    );
    let (rules, used) = set.into_content_blocking().unwrap_or_default();
    let lists = split(rules);
    let stats = Stats {
        lists: sources.len(),
        network,
        converted: used.len(),
        rules: lists.iter().map(|l| l.len()).sum(),
        webkit_lists: lists.len(),
        took: started.elapsed(),
        ..Default::default()
    };
    let json = lists
        .iter()
        .map(|l| serde_json::to_string(l).unwrap_or_else(|_| "[]".into()))
        .collect();
    (json, stats)
}

/// Whether `line` is a network filter (not a comment, a cosmetic filter or a directive).
fn is_network_line(line: &str) -> bool {
    let l = line.trim();
    !(l.is_empty()
        || l.starts_with('!')
        || l.starts_with('[')
        || l.starts_with('#')
        || l.contains("##")
        || l.contains("#@#")
        || l.contains("#?#")
        || l.contains("#$#")
        || l.contains("#%#"))
}

/// `rules` (exceptions last, as `into_content_blocking` orders them) split into lists of at most
/// [`MAX_RULES`], each list's blocks followed by every exception.
pub fn split(rules: Vec<CbRule>) -> Vec<Vec<CbRule>> {
    let (exceptions, blocks): (Vec<CbRule>, Vec<CbRule>) = rules
        .into_iter()
        .partition(|r| matches!(r.action.typ, CbType::IgnorePreviousRules));
    if blocks.is_empty() {
        return vec![];
    }
    let room = MAX_RULES.saturating_sub(exceptions.len()).max(1000);
    blocks
        .chunks(room)
        .map(|chunk| {
            let mut list = chunk.to_vec();
            list.extend(exceptions.iter().cloned());
            list
        })
        .collect()
}

/// The engine for pages: every filter of `sources` and the user's `user` lines, with uBlock
/// Origin's scriptlets and redirect resources (`resources`).
pub fn engine(
    sources: &[Source],
    user: &str,
    resources: Vec<adblock::resources::Resource>,
) -> Engine {
    let mut set = FilterSet::new(false);
    for s in sources {
        set.add_filter_list(s.text.clone(), options(s, RuleTypes::All));
    }
    let user_source = Source {
        list: None,
        text: user.to_string(),
    };
    set.add_filter_list(user.to_string(), options(&user_source, RuleTypes::All));
    let mut engine = Engine::new_with_filter_set(set);
    engine.use_resources(resources);
    engine
}

/// The engine for pages, with the names of the scriptlet functions uBlock Origin runs in an
/// isolated world (its `world: 'ISOLATED'`), which Trek runs in its own world, not the page's.
pub struct Pages {
    pub engine: Engine,
    pub isolated: std::collections::HashSet<String>,
}

/// What a page on `url` gets: the site's selectors to hide, its procedural filters, the
/// exceptions for generic hiding, whether generic hiding is off, and the scriptlets' code.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub hide: Vec<String>,
    pub procedural: Vec<String>,
    pub exceptions: Vec<String>,
    pub generichide: bool,
    pub script: String,
}

/// `page`, with the scriptlets split by the world each runs in: the page's own, and Trek's
/// isolated one (`isolated_script`).
pub fn page_split(pages: &Pages, url: &str) -> (Page, String) {
    let mut p = page(&pages.engine, url);
    let (main, isolated) = split_worlds(&p.script, &pages.isolated);
    p.script = main;
    (p, isolated)
}

/// Split the engine's script (its dependencies' sources, then one `try { name(args) } catch`
/// call per scriptlet, as the `adblock` crate puts them together) into the part for the page's
/// world and the part for an isolated world: each call goes where its scriptlet runs, after the
/// dependencies (both parts get them: only the calls run anything).
pub fn split_worlds(
    script: &str,
    isolated: &std::collections::HashSet<String>,
) -> (String, String) {
    const OPEN: &str = "try {\n";
    const CLOSE: &str = "\n} catch ( e ) { }\n";
    let mut rest = script;
    let mut calls: Vec<&str> = vec![];
    // The calls are at the end, each a whole block of its own: taken off from the end.
    while rest.ends_with(CLOSE) {
        let body_end = rest.len() - CLOSE.len();
        let Some(open) = rest[..body_end].rfind(OPEN) else {
            break;
        };
        let call = &rest[open + OPEN.len()..body_end];
        // One line, a function's name and its arguments.
        if call.contains('\n') || !call.ends_with(')') {
            break;
        }
        calls.push(&rest[open..]);
        rest = &rest[..open];
    }
    if calls.is_empty() {
        return (script.to_string(), String::new());
    }
    calls.reverse();
    let name_of = |block: &str| -> String {
        block[OPEN.len()..]
            .split('(')
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let (iso, main): (Vec<&str>, Vec<&str>) = calls
        .into_iter()
        .partition(|b| isolated.contains(&name_of(b)));
    let part = |blocks: Vec<&str>| {
        if blocks.is_empty() {
            String::new()
        } else {
            format!("{rest}{}", blocks.concat())
        }
    };
    (part(main), part(iso))
}

pub fn page(engine: &Engine, url: &str) -> Page {
    let r = engine.url_cosmetic_resources(url);
    let mut hide: Vec<String> = r.hide_selectors.into_iter().collect();
    hide.sort();
    let mut procedural: Vec<String> = r.procedural_actions.into_iter().collect();
    procedural.sort();
    let mut exceptions: Vec<String> = r.exceptions.into_iter().collect();
    exceptions.sort();
    Page {
        hide,
        procedural,
        exceptions,
        generichide: r.generichide,
        script: r.injected_script,
    }
}

/// The generic selectors to hide for the page's `classes` and `ids` (but not its `exceptions`).
pub fn generic(
    engine: &Engine,
    classes: &[String],
    ids: &[String],
    exceptions: &[String],
) -> Vec<String> {
    let exceptions: std::collections::HashSet<String> = exceptions.iter().cloned().collect();
    engine.hidden_class_id_selectors(classes, ids, &exceptions)
}

/// The scriptlets' code as it runs in the page: in a function of its own, with the global
/// uBlock Origin's scriptlets share (`scriptletGlobals`).
pub fn wrap_script(script: &str) -> String {
    if script.trim().is_empty() {
        return String::new();
    }
    format!(
        "/*trek-adblock*/\n(function () {{\n'use strict';\nconst scriptletGlobals = {{}};\ntry {{\n{script}\n}} catch (e) {{}}\n}})();\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> Source {
        Source {
            list: None,
            text: text.into(),
        }
    }

    #[test]
    fn network_filters_become_webkit_rules_with_their_exceptions() {
        let s = [source(
            "||ads.example.com^\n||tracker.example.net^$third-party\n@@||ads.example.com/ok.js\nexample.com##.banner\n",
        )];
        let (lists, stats) = content_rules(&s, "");
        assert_eq!(lists.len(), 1);
        assert_eq!(stats.network, 3);
        let rules: Vec<serde_json::Value> = serde_json::from_str(&lists[0]).unwrap();
        // Blocks first, the exception after them.
        let kinds: Vec<&str> = rules
            .iter()
            .map(|r| r["action"]["type"].as_str().unwrap())
            .collect();
        let last_block = kinds.iter().rposition(|k| *k == "block").unwrap();
        let first_exception = kinds
            .iter()
            .position(|k| *k == "ignore-previous-rules")
            .unwrap();
        assert!(last_block < first_exception, "{kinds:?}");
        // Cosmetic filters aren't WebKit rules: the page gets them.
        assert!(!kinds.contains(&"css-display-none"));
    }

    #[test]
    fn a_list_too_big_for_webkit_is_split_and_each_part_keeps_the_exceptions() {
        let block = |i: usize| CbRule {
            action: adblock::content_blocking::CbAction {
                typ: CbType::Block,
                selector: None,
            },
            trigger: adblock::content_blocking::CbTrigger {
                url_filter: format!("^https?://ad{i}\\.example/"),
                ..Default::default()
            },
        };
        let mut rules: Vec<CbRule> = (0..MAX_RULES + 10).map(block).collect();
        let mut exception = block(0);
        exception.action.typ = CbType::IgnorePreviousRules;
        rules.push(exception);
        let lists = split(rules);
        assert_eq!(lists.len(), 2);
        assert!(lists.iter().all(|l| l.len() <= MAX_RULES));
        for l in &lists {
            assert!(matches!(
                l.last().unwrap().action.typ,
                CbType::IgnorePreviousRules
            ));
        }
    }

    #[test]
    fn a_page_gets_its_site_s_hiding_and_procedural_filters() {
        let s = [source(
            "example.com##.ad-box\nexample.com##div:has-text(Sponsored)\n##.generic-ad\nexample.com#@#.generic-ad\nother.org##.not-here\n",
        )];
        let engine = engine(&s, "", vec![]);
        let p = page(&engine, "https://www.example.com/news");
        assert!(p.hide.contains(&".ad-box".to_string()), "{p:?}");
        assert!(!p.hide.contains(&".not-here".to_string()));
        assert_eq!(p.procedural.len(), 1);
        assert!(p.procedural[0].contains("has-text"));
        // Generic hiding by class, with the site's exception kept out.
        let none = generic(&engine, &["generic-ad".into()], &[], &p.exceptions);
        assert!(none.is_empty(), "{none:?}");
        let elsewhere = page(&engine, "https://elsewhere.test/");
        let some = generic(&engine, &["generic-ad".into()], &[], &elsewhere.exceptions);
        assert_eq!(some, vec![".generic-ad".to_string()]);
    }

    #[test]
    fn scriptlets_run_from_the_resources() {
        use base64::Engine as _;
        let resource =
            |name: &str, kind, deps: Vec<&str>, src: &str| adblock::resources::Resource {
                name: name.into(),
                aliases: vec![],
                kind: adblock::resources::ResourceType::Mime(kind),
                content: base64::engine::general_purpose::STANDARD.encode(src),
                dependencies: deps.into_iter().map(String::from).collect(),
                permission: Default::default(),
            };
        use adblock::resources::MimeType;
        let engine = engine(
            &[source("example.com##+js(set-flag, ads, 0)\n")],
            "",
            vec![
                resource(
                    "helper.fn",
                    MimeType::FnJavascript,
                    vec![],
                    "function helper() { return 1; }",
                ),
                resource(
                    "set-flag.js",
                    MimeType::ApplicationJavascript,
                    vec!["helper.fn"],
                    "function setFlag(name, value) { window[name] = value + helper(); }",
                ),
            ],
        );
        let p = page(&engine, "https://example.com/");
        assert!(p.script.contains("function helper()"), "{}", p.script);
        assert!(p.script.contains("setFlag(\"ads\", \"0\")"), "{}", p.script);
        let wrapped = wrap_script(&p.script);
        assert!(wrapped.contains("const scriptletGlobals = {};"));
        assert_eq!(wrap_script("  "), "");
    }

    #[test]
    fn the_user_s_own_filters_hide_run_and_keep_the_lists_key() {
        let lists = [source("||ads.example^\n")];
        let mine = "shop.test##.promo\nshop.test##div.card:has-text(Buy now)\nshop.test##+js(set-flag, ads, 0)";
        // Elements the user picked change no WebKit rule: the compiled lists are kept.
        assert_eq!(key(&lists, ""), key(&lists, mine));
        assert_ne!(key(&lists, ""), key(&lists, "||tracker.test^"));
        use base64::Engine as _;
        let set_flag = adblock::resources::Resource {
            name: "set-flag.js".into(),
            aliases: vec![],
            kind: adblock::resources::ResourceType::Mime(
                adblock::resources::MimeType::ApplicationJavascript,
            ),
            content: base64::engine::general_purpose::STANDARD
                .encode("function setFlag(n, v) { window[n] = v; }"),
            dependencies: vec![],
            permission: Default::default(),
        };
        let engine = engine(&lists, mine, vec![set_flag]);
        let p = page(&engine, "https://shop.test/");
        assert!(p.hide.contains(&".promo".to_string()), "{p:?}");
        assert_eq!(p.procedural.len(), 1, "{p:?}");
        assert!(p.script.contains("setFlag(\"ads\", \"0\")"), "{}", p.script);
    }

    #[test]
    fn isolated_scriptlets_are_kept_out_of_the_page_s_world() {
        let script = "function dep() { try {\nx()\n} catch ( e ) { }\n }\nfunction a() {}\nfunction b() {}\n\
                      try {\na(\"1\")\n} catch ( e ) { }\ntry {\nb(\"2\")\n} catch ( e ) { }\n";
        let isolated: std::collections::HashSet<String> = ["b".to_string()].into();
        let (main, iso) = split_worlds(script, &isolated);
        assert!(
            main.contains("a(\"1\")") && !main.contains("b(\"2\")"),
            "{main}"
        );
        assert!(
            iso.contains("b(\"2\")") && !iso.contains("a(\"1\")"),
            "{iso}"
        );
        // The dependencies (one with a block like a call inside it) go with both.
        assert!(main.starts_with("function dep()") && iso.starts_with("function dep()"));
        let (all, none) = split_worlds(script, &Default::default());
        assert_eq!(all, script);
        assert_eq!(none, "");
        let (only_main, _) = split_worlds("no calls here", &isolated);
        assert_eq!(only_main, "no calls here");
    }

    #[test]
    fn hosts_files_are_read_as_hosts() {
        let s = [Source {
            list: lists::DEFAULTS.iter().find(|l| l.hosts),
            text: "127.0.0.1 adserver.example\n0.0.0.0 tracker.example\n".into(),
        }];
        let (lists, stats) = content_rules(&s, "");
        assert_eq!(lists.len(), 1);
        assert!(stats.converted >= 2, "{stats:?}");
        assert!(lists[0].contains("adserver"));
    }
}
