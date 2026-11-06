//! What each event says, as PostHog's capture API takes it. Every event and its properties are
//! listed in docs/PRIVACY.md: change one here, change it there.

use serde_json::{Map, Value, json};

/// Where Trek runs, as every usage event says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    /// `macOS`, `Windows`, `Linux`.
    pub os: &'static str,
    /// `26.1`, `11 (26100)`: the version alone.
    pub os_version: Option<String>,
    /// `aarch64`, `x86_64`.
    pub arch: &'static str,
    pub app_version: String,
    /// `stable`, `beta`, `nightly`.
    pub channel: String,
}

impl Platform {
    pub fn here(os_version: Option<String>, channel: &str) -> Platform {
        let os = match std::env::consts::OS {
            "macos" => "macOS",
            "windows" => "Windows",
            "linux" => "Linux",
            other => other,
        };
        Platform {
            os,
            os_version,
            arch: std::env::consts::ARCH,
            app_version: trek_core::VERSION.to_string(),
            channel: channel.to_string(),
        }
    }

    /// "macOS 26.1, aarch64".
    pub fn describe(&self) -> String {
        match &self.os_version {
            Some(v) => format!("{} {v}, {}", self.os, self.arch),
            None => format!("{}, {}", self.os, self.arch),
        }
    }
}

/// What a usage or feature count is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    /// An agent a message was sent to (`claude-code`, `codex`, `direct:anthropic`…).
    Agent,
    /// A tool opened in the tools panel (`browser`, `terminal`, `git`…).
    Tool,
}

impl Feature {
    pub fn key(self) -> &'static str {
        match self {
            Feature::Agent => "agent",
            Feature::Tool => "tool",
        }
    }

    pub fn from_key(key: &str) -> Option<Feature> {
        match key {
            "agent" => Some(Feature::Agent),
            "tool" => Some(Feature::Tool),
            _ => None,
        }
    }
}

/// One thing Trek tells PostHog.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The first launch with telemetry on, once per install id. `set_up_before`: Trek was
    /// already set up in this data folder (an existing user's first count, not a new install).
    Install { set_up_before: bool },
    /// Trek was opened: at most once a day.
    Opened,
    /// Trek runs a version it hadn't before.
    Updated { from: String, to: String },
    /// How often something was used on `day` (`YYYY-MM-DD`): no names but Trek's own.
    Used {
        feature: Feature,
        name: String,
        count: u32,
        day: String,
    },
    /// A panic of an earlier run, with what could name the user already taken out.
    Crash(trek_logging::CrashReport),
}

/// A backtrace is cut to this many characters.
const BACKTRACE_MAX: usize = 16_000;

impl Event {
    pub fn name(&self) -> &'static str {
        match self {
            Event::Install { .. } => "install",
            Event::Opened => "app_opened",
            Event::Updated { .. } => "app_updated",
            Event::Used { .. } => "feature_used",
            // PostHog's own name for an error, so crashes show in its error tracking.
            Event::Crash(_) => "$exception",
        }
    }

    pub fn properties(&self, platform: &Platform) -> Map<String, Value> {
        let mut p = base(Some(platform));
        match self {
            Event::Install { set_up_before } => {
                p.insert("set_up_before".into(), json!(set_up_before));
            }
            Event::Opened => {}
            Event::Updated { from, to } => {
                p.insert("from_version".into(), json!(from));
                p.insert("to_version".into(), json!(to));
            }
            Event::Used {
                feature,
                name,
                count,
                day,
            } => {
                p.insert("kind".into(), json!(feature.key()));
                p.insert("name".into(), json!(name));
                p.insert("count".into(), json!(count));
                p.insert("day".into(), json!(day));
            }
            Event::Crash(report) => {
                let mut backtrace = report.backtrace.clone();
                if let Some((cut, _)) = backtrace.char_indices().nth(BACKTRACE_MAX) {
                    backtrace.truncate(cut);
                    backtrace.push_str("\n…");
                }
                p.insert(
                    "$exception_list".into(),
                    json!([{
                        "type": "panic",
                        "value": report.message,
                        "mechanism": { "type": "panic", "handled": false },
                    }]),
                );
                p.insert("$exception_level".into(), json!("fatal"));
                p.insert("location".into(), json!(report.location));
                p.insert("thread".into(), json!(report.thread));
                p.insert("backtrace".into(), json!(backtrace));
                p.insert("crashed_version".into(), json!(report.version));
            }
        }
        p
    }
}

/// What every event carries: never a person, never a place. `platform`: the version, OS and
/// architecture, which feedback without diagnostics leaves out.
pub fn base(platform: Option<&Platform>) -> Map<String, Value> {
    let mut p = Map::new();
    p.insert("$lib".into(), json!("trek"));
    // An anonymous event: no person profile is made from the id.
    p.insert("$process_person_profile".into(), json!(false));
    // No country or city from the address it came from.
    p.insert("$geoip_disable".into(), json!(true));
    if let Some(platform) = platform {
        p.insert("$app_version".into(), json!(platform.app_version));
        p.insert("channel".into(), json!(platform.channel));
        p.insert("$os".into(), json!(platform.os));
        if let Some(v) = &platform.os_version {
            p.insert("$os_version".into(), json!(v));
        }
        p.insert("arch".into(), json!(platform.arch));
    }
    p
}

/// An event as the batch API takes it. `uuid` lets PostHog drop a copy a retry sent again.
pub fn envelope(
    name: &str,
    properties: Map<String, Value>,
    distinct_id: &str,
    at: chrono::DateTime<chrono::Utc>,
) -> Value {
    json!({
        "event": name,
        "distinct_id": distinct_id,
        "properties": properties,
        "timestamp": at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "uuid": uuid::Uuid::now_v7().to_string(),
    })
}

/// The body a batch is posted with.
pub fn batch_body(api_key: &str, events: Vec<Value>) -> Value {
    json!({ "api_key": api_key, "batch": events })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn platform() -> Platform {
        Platform {
            os: "macOS",
            os_version: Some("26.1".into()),
            arch: "aarch64",
            app_version: "0.4.41".into(),
            channel: "stable".into(),
        }
    }

    #[test]
    fn every_event_is_anonymous_and_says_where_it_runs() {
        let crash = trek_logging::CrashReport {
            at: 1,
            version: "0.4.40".into(),
            thread: "main".into(),
            location: "crates/x/src/lib.rs:3".into(),
            message: "boom".into(),
            backtrace: "   0: x\n".into(),
        };
        for event in [
            Event::Install {
                set_up_before: true,
            },
            Event::Opened,
            Event::Updated {
                from: "0.4.40".into(),
                to: "0.4.41".into(),
            },
            Event::Used {
                feature: Feature::Agent,
                name: "codex".into(),
                count: 3,
                day: "2026-11-05".into(),
            },
            Event::Crash(crash),
        ] {
            let p = event.properties(&platform());
            assert_eq!(p["$process_person_profile"], json!(false), "{event:?}");
            assert_eq!(p["$geoip_disable"], json!(true), "{event:?}");
            assert_eq!(
                (&p["$app_version"], &p["$os"], &p["$os_version"], &p["arch"]),
                (
                    &json!("0.4.41"),
                    &json!("macOS"),
                    &json!("26.1"),
                    &json!("aarch64")
                ),
            );
        }
    }

    #[test]
    fn the_payloads_have_the_shapes_posthog_takes() {
        let updated = Event::Updated {
            from: "0.4.40".into(),
            to: "0.4.41".into(),
        };
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-05T10:00:00Z")
            .unwrap()
            .to_utc();
        let e = envelope(updated.name(), updated.properties(&platform()), "id-1", at);
        assert_eq!(e["event"], "app_updated");
        assert_eq!(e["distinct_id"], "id-1");
        assert_eq!(e["timestamp"], "2026-11-05T10:00:00.000Z");
        assert_eq!(e["properties"]["from_version"], "0.4.40");
        assert_eq!(e["properties"]["to_version"], "0.4.41");
        assert!(uuid::Uuid::parse_str(e["uuid"].as_str().unwrap()).is_ok());
        let body = batch_body("phc_x", vec![e.clone(), e]);
        assert_eq!(body["api_key"], "phc_x");
        assert_eq!(body["batch"].as_array().unwrap().len(), 2);

        let used = Event::Used {
            feature: Feature::Tool,
            name: "browser".into(),
            count: 2,
            day: "2026-11-04".into(),
        };
        let p = used.properties(&platform());
        assert_eq!(
            (&p["kind"], &p["name"], &p["count"], &p["day"]),
            (
                &json!("tool"),
                &json!("browser"),
                &json!(2),
                &json!("2026-11-04")
            )
        );

        let crash = Event::Crash(trek_logging::CrashReport {
            at: 1,
            version: "0.4.40".into(),
            thread: "main".into(),
            location: "crates/x/src/lib.rs:3".into(),
            message: "index out of bounds".into(),
            backtrace: "x".repeat(BACKTRACE_MAX + 50),
        });
        assert_eq!(crash.name(), "$exception");
        let p = crash.properties(&platform());
        assert_eq!(p["$exception_list"][0]["type"], "panic");
        assert_eq!(p["$exception_list"][0]["value"], "index out of bounds");
        assert_eq!(p["$exception_list"][0]["mechanism"]["handled"], false);
        assert_eq!(p["crashed_version"], "0.4.40");
        assert!(p["backtrace"].as_str().unwrap().chars().count() <= BACKTRACE_MAX + 2);
    }

    #[test]
    fn feedback_without_diagnostics_says_nothing_of_the_machine() {
        let p = base(None);
        assert!(
            !p.contains_key("$os") && !p.contains_key("$app_version") && !p.contains_key("arch")
        );
        assert_eq!(p["$process_person_profile"], json!(false));
    }
}
