//! What a failed model request means, and what to do about it: wait and try again, make room in
//! the context, or stop and say why. One blip used to end the whole answer; a rejected request
//! used to be sent again without its tools, which a model then "answered" with no way to act.

use std::time::Duration;

/// A failed model request, classified.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestFailure {
    pub kind: FailureKind,
    pub message: String,
    /// How long the provider asked to wait.
    pub retry_after: Option<Duration>,
    /// The usage limit this is, if tries run out (a rate limit, waited out first).
    pub limit: Option<crate::limits::Limit>,
}

impl std::fmt::Display for RequestFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RequestFailure {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Worth sending again: a dropped connection, a busy server, a rate limit with a short wait.
    Transient,
    /// The conversation doesn't fit the model's window: making room and sending again can work.
    ContextOverflow,
    /// Out of credit, a bad key, a model that doesn't exist.
    Fatal,
    /// The provider won't take this request, however often it's sent (a 400 or 422).
    Refused,
}

impl RequestFailure {
    pub fn transient(message: impl Into<String>) -> Self {
        RequestFailure {
            kind: FailureKind::Transient,
            message: message.into(),
            retry_after: None,
            limit: None,
        }
    }

    pub fn is_retryable(&self) -> bool {
        self.kind == FailureKind::Transient
    }
}

/// How many times a request is sent before giving up on it.
pub const MAX_ATTEMPTS: u32 = 8;
/// The longest a provider's own `Retry-After` is waited out for. Past this it's a usage limit: Trek
/// says when it resets instead of holding the turn open.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(120);

/// How long to wait before attempt `attempt + 1` (`attempt` counted from 1): 2, 4, 8, then 10
/// seconds, the provider's own `Retry-After` being a floor.
pub fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    let own = Duration::from_secs(match attempt {
        0 | 1 => 2,
        2 => 4,
        3 => 8,
        _ => 10,
    });
    match retry_after {
        // However long a busy server asks for, an answer isn't held open longer than this.
        Some(asked) => asked.max(own).min(MAX_RETRY_AFTER),
        None => own,
    }
}

/// The `Retry-After` and `retry-after-ms` headers, as a wait.
pub fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    if let Some(ms) = text("retry-after-ms")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|ms| ms.is_finite())
    {
        return Some(Duration::from_secs_f64(ms.clamp(0.0, 1e12) / 1000.0));
    }
    let value = text("retry-after")?.trim();
    if let Ok(seconds) = value
        .parse::<f64>()
        .map(|s| if s.is_finite() { s } else { 0.0 })
    {
        return Some(Duration::from_secs_f64(seconds.clamp(0.0, 1e9)));
    }
    let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    (at.with_timezone(&chrono::Utc) - chrono::Utc::now())
        .to_std()
        .ok()
}

/// Whether `message` says the prompt doesn't fit the model's context window. The wordings are
/// each provider's own; "too many tokens per minute" and the like are rate limits and aren't here.
pub fn is_context_overflow(message: &str) -> bool {
    let m = message.to_lowercase();
    const SAYS: &[&str] = &[
        "prompt is too long",
        "input is too long",
        "exceeds the context window",
        "exceeds the maximum context",
        "exceeds maximum context",
        "exceeds the model's maximum context",
        "exceeds model's maximum context",
        "maximum context length",
        "context_length_exceeded",
        "context length exceeded",
        "model_context_window_exceeded",
        "context window exceeds",
        "reduce the length of the messages",
        "too large for model",
        "string too long",
        "request_too_large",
        "request entity too large",
        "exceeds the available context",
        "greater than the context length",
        "maximum prompt length",
        "exceeds max length",
        "prompt too long",
        "input token count exceeds",
        "input_tokens exceed",
    ];
    SAYS.iter().any(|s| m.contains(s))
}

/// Whether `message` is about an account's credit, not the moment: never worth sending again.
fn is_out_of_credit(message: &str) -> bool {
    let m = message.to_lowercase();
    ["insufficient", "credit", "quota", "billing", "payment"]
        .iter()
        .any(|s| m.contains(s))
        // OpenRouter's "retry after in-flight requests settle" clears by itself.
        && !m.contains("in-flight")
}

/// A response with `status` and error text `message`, classified.
pub fn classify_status(
    status: u16,
    message: &str,
    retry_after: Option<Duration>,
) -> RequestFailure {
    let kind = match status {
        _ if is_context_overflow(message) => FailureKind::ContextOverflow,
        413 => FailureKind::ContextOverflow,
        // A request may simply have collided with others spending the same credit.
        402 if message.to_lowercase().contains("in-flight") => FailureKind::Transient,
        400 | 422 => FailureKind::Refused,
        401 | 402 | 403 | 404 | 405 => FailureKind::Fatal,
        // A rate limit is a wait. A long wait, or being out of quota, is a usage limit.
        429 => {
            if is_out_of_credit(message) || retry_after.is_some_and(|w| w > MAX_RETRY_AFTER) {
                FailureKind::Fatal
            } else {
                FailureKind::Transient
            }
        }
        408 | 409 | 425 => FailureKind::Transient,
        500..=599 => FailureKind::Transient,
        _ => FailureKind::Transient,
    };
    RequestFailure {
        kind,
        message: message.to_string(),
        retry_after,
        limit: None,
    }
}

/// An error a provider reported inside a 200 stream: `{"error":{"code":…,"message":…}}` where
/// `code` may be an HTTP status, a provider's word, or absent.
pub fn classify_stream_error(error: &serde_json::Value) -> RequestFailure {
    let message = error["message"]
        .as_str()
        .or(error.as_str())
        .unwrap_or("the provider reported an error")
        .to_string();
    if let Some(status) = error["code"].as_u64().and_then(|c| u16::try_from(c).ok()) {
        if (100..600).contains(&status) {
            return classify_status(status, &message, None);
        }
    }
    let code = error["code"]
        .as_str()
        .or(error["type"].as_str())
        .unwrap_or_default()
        .to_lowercase();
    if is_context_overflow(&message) || is_context_overflow(&code) {
        return RequestFailure {
            kind: FailureKind::ContextOverflow,
            message,
            retry_after: None,
            limit: None,
        };
    }
    if code.contains("auth")
        || code.contains("invalid_request")
        || code.contains("permission")
        || is_out_of_credit(&message)
    {
        return RequestFailure {
            kind: FailureKind::Fatal,
            message,
            retry_after: None,
            limit: None,
        };
    }
    // "Provider returned error", "overloaded", "server_error", "timeout": the next try usually works.
    RequestFailure::transient(message)
}

/// Whether a refusal blames something already in the conversation: a tool call, a picture, a
/// model's reasoning. Then every later request fails on it too, and a fresh context is the way out.
pub fn blames_history(message: &str) -> bool {
    let m = message.to_lowercase();
    [
        "tool_call",
        "tool call",
        "tool_use",
        "tool_result",
        "function_call",
        "image",
        "reasoning",
        "thinking",
        "signature",
        "messages",
    ]
    .iter()
    .any(|w| m.contains(w))
}

/// Whether a 400 says the model takes no `tools`. Only then may a request go out without them: a
/// model with no tools can't act, so nothing else is reason enough to take them away.
pub fn says_no_tools(message: &str) -> bool {
    let m = message.to_lowercase();
    (m.contains("tool") || m.contains("function call") || m.contains("function_call"))
        && (m.contains("not support")
            || m.contains("n't support")
            || m.contains("unsupported")
            || m.contains("not available")
            || m.contains("no endpoints found that support")
            // llama.cpp: "tools param requires --jinja flag".
            || m.contains("--jinja"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn overflow_wordings_from_each_provider_are_recognised() {
        for m in [
            "prompt is too long: 214000 tokens > 200000 maximum",
            "This model's maximum context length is 128000 tokens. However, you requested 140000",
            "Your input exceeds the context window of this model",
            "400 Input is too long for requested model.",
            "context_length_exceeded",
            "Please reduce the length of the messages or completion.",
            "Request too large for model with 8192 maximum context length",
        ] {
            assert!(is_context_overflow(m), "{m}");
        }
        for m in [
            "Rate limit reached: too many tokens per minute",
            "Provider returned error",
            "This endpoint's maximum output is 8192",
        ] {
            assert!(!is_context_overflow(m), "{m}");
        }
    }

    #[test]
    fn statuses_are_classified_by_what_helps() {
        let kind = |s, m: &str| classify_status(s, m, None).kind;
        assert_eq!(kind(503, "overloaded"), FailureKind::Transient);
        assert_eq!(kind(500, "oops"), FailureKind::Transient);
        assert_eq!(kind(429, "slow down"), FailureKind::Transient);
        assert_eq!(kind(408, "timeout"), FailureKind::Transient);
        assert_eq!(kind(401, "bad key"), FailureKind::Fatal);
        assert_eq!(kind(404, "no such model"), FailureKind::Fatal);
        assert_eq!(kind(400, "bad history"), FailureKind::Refused);
        assert!(blames_history("messages.3: tool_use ids must be unique"));
        assert!(!blames_history("Invalid model"));
        assert_eq!(
            kind(400, "maximum context length is 8192 tokens"),
            FailureKind::ContextOverflow
        );
        assert_eq!(kind(413, "too big"), FailureKind::ContextOverflow);
        assert_eq!(
            kind(
                402,
                "You requested up to 131072 tokens, but can only afford 18354"
            ),
            FailureKind::Fatal
        );
        assert_eq!(
            kind(
                402,
                "This request would exceed your available credits given your current in-flight requests. Retry after in-flight requests settle"
            ),
            FailureKind::Transient
        );
        assert_eq!(
            kind(429, "You exceeded your current quota"),
            FailureKind::Fatal
        );
    }

    #[test]
    fn a_long_retry_after_is_a_usage_limit_not_a_wait() {
        let long = classify_status(429, "slow", Some(Duration::from_secs(3600)));
        assert_eq!(long.kind, FailureKind::Fatal);
        let short = classify_status(429, "slow", Some(Duration::from_secs(30)));
        assert_eq!(short.kind, FailureKind::Transient);
    }

    #[test]
    fn errors_inside_a_stream_are_classified_too() {
        let kind = |e| classify_stream_error(&e).kind;
        assert_eq!(
            kind(json!({"code":502,"message":"Provider returned error"})),
            FailureKind::Transient
        );
        assert_eq!(
            kind(json!({"message":"upstream connect error"})),
            FailureKind::Transient
        );
        assert_eq!(
            kind(json!({"code":"context_length_exceeded","message":"too long"})),
            FailureKind::ContextOverflow
        );
        assert_eq!(
            kind(json!({"code":401,"message":"bad key"})),
            FailureKind::Fatal
        );
    }

    #[test]
    fn backoff_grows_and_respects_the_provider() {
        assert_eq!(backoff(1, None), Duration::from_secs(2));
        assert_eq!(backoff(2, None), Duration::from_secs(4));
        assert_eq!(backoff(3, None), Duration::from_secs(8));
        assert_eq!(backoff(7, None), Duration::from_secs(10));
        assert_eq!(
            backoff(1, Some(Duration::from_secs(30))),
            Duration::from_secs(30)
        );
        assert_eq!(
            backoff(3, Some(Duration::from_secs(1))),
            Duration::from_secs(8)
        );
    }

    #[test]
    fn retry_after_reads_seconds_and_milliseconds() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("retry-after", "7".parse().unwrap());
        assert_eq!(retry_after(&h), Some(Duration::from_secs(7)));
        h.insert("retry-after-ms", "1500".parse().unwrap());
        assert_eq!(retry_after(&h), Some(Duration::from_millis(1500)));
        // Parses as a float, and must not panic on its way to a Duration.
        let mut odd = reqwest::header::HeaderMap::new();
        for value in ["NaN", "inf", "-5"] {
            odd.insert("retry-after", value.parse().unwrap());
            assert_eq!(retry_after(&odd), Some(Duration::ZERO), "{value}");
        }
        odd.remove("retry-after");
        odd.insert("retry-after-ms", "NaN".parse().unwrap());
        assert_eq!(retry_after(&odd), None);
    }

    #[test]
    fn only_a_message_about_tools_may_take_them_away() {
        assert!(says_no_tools("This model does not support tools"));
        assert!(says_no_tools("No endpoints found that support tool use"));
        assert!(!says_no_tools("messages: invalid tool_call_id"));
        assert!(!says_no_tools("maximum context length exceeded"));
    }
}
