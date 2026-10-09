//! Direct providers (API keys and local servers). Phase 1 is streaming chat with full history;
//! Trek's own tool loop (read/edit/bash with the hand-holding gates) lands in phase 2.

use crate::{AgentEvent, Billing, Command, SessionConfig, load_image};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;
use anyhow::{Context as _, Result, bail};
use futures::StreamExt;
use serde_json::{Value, json};
use trek_core::catalog::{Wire, direct_provider};
use trek_core::settings::secrets;
use trek_core::{AgentId, Effort, TokenUsage, UsageCost};

/// The error for a failed request: a usage limit for a 429 (said with `AgentEvent::LimitReached`),
/// else `message` as it is.
fn request_error(status: reqwest::StatusCode, headers: &reqwest::header::HeaderMap, message: String) -> anyhow::Error {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return crate::limits::LimitError(crate::limits::from_response(headers, message, trek_core::store::now_ms())).into();
    }
    anyhow::anyhow!(message)
}

const SYSTEM: &str = "You are Trek, a coding assistant. Be direct and concise. Use Markdown with fenced code blocks.";
const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";

pub async fn run(
    config: SessionConfig,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    let AgentId::Direct(provider_id) = &config.agent else { unreachable!() };
    let provider = direct_provider(provider_id).with_context(|| format!("unknown provider {provider_id}"))?;
    let key = secrets::api_key(provider.id);
    if !provider.local && key.is_none() {
        bail!("No API key for {}. Add one in Settings → API Keys.", provider.name);
    }
    let mut model = config.model.clone().context("Pick a model first")?;
    let mut effort = config.effort;
    let client = reqwest::Client::new();
    events
        .send(AgentEvent::Started { native_id: config.resume.clone().unwrap_or_default(), model: Some(model.clone()) })
        .await?;
    events.send(AgentEvent::Billing(if provider.local { Billing::Local } else { Billing::Metered })).await?;

    // Anthropic: raw assistant content blocks are kept and replayed unchanged (append-only).
    let mut history: Vec<Value> = Vec::new();
    let mut kept = VecDeque::new();
    let mut shutdown = false;

    loop {
        let cmd = match kept.pop_front() {
            Some(cmd) => cmd,
            None => match commands.recv().await {
                Ok(cmd) => cmd,
                Err(_) => break,
            },
        };
        match cmd {
            Command::Prompt { text, images } => {
                let mut next = Some((text, images));
                let mut error = None;
                while let Some((text, images)) = next.take() {
                    let (message, skipped) = user_message(provider.wire, &text, &images);
                    for e in skipped {
                        events.send(AgentEvent::Notice(format!("Image left out: {e}"))).await?;
                    }
                    history.push(message);
                    let result = match provider.wire {
                        Wire::Anthropic => {
                            anthropic_turn(&client, ANTHROPIC_MESSAGES_URL, key.as_deref().unwrap_or_default(), &model, effort, &history, &events, &commands, &mut kept, &mut shutdown).await
                        }
                        _ => {
                            let usage = REPORTS_USAGE.contains(&provider.id);
                            openai_turn(
                                &client,
                                &config.agent,
                                provider.base_url,
                                key.as_deref(),
                                provider.local,
                                usage,
                                &model,
                                effort,
                                &history,
                                &events,
                                &commands,
                                &mut kept,
                                &mut shutdown,
                            )
                            .await
                        }
                    };
                    match result {
                        Ok(assistant) => history.push(assistant),
                        Err(e) => {
                            history.pop();
                            if let Some(crate::limits::LimitError(limit)) = e.downcast_ref() {
                                events.send(limit.clone().event()).await?;
                            }
                            error.get_or_insert_with(|| format!("{e:#}"));
                        }
                    }
                    while let Some(cmd) = kept.pop_front() {
                        match cmd {
                            Command::Prompt { text, images } => {
                                next = Some((text, images));
                                break;
                            }
                            Command::SetModel { model: m, effort: e } => {
                                model = m;
                                effort = e;
                            }
                            Command::SetModes { effort: e, .. } => effort = e,
                            Command::Shutdown => {
                                shutdown = true;
                                break;
                            }
                            Command::Answer { .. }
                            | Command::Interrupt
                            | Command::Respond { .. }
                            | Command::SetHandHolding(_)
                            | Command::ReadTask { .. }
                            | Command::StopTask { .. } => {}
                        }
                    }
                    if shutdown {
                        break;
                    }
                }
                events.send(AgentEvent::TurnComplete { error }).await?;
                if shutdown {
                    break;
                }
            }
            Command::SetModel { model: m, effort: e } => {
                model = m;
                effort = e;
            }
            Command::SetModes { effort: e, .. } => effort = e,
            Command::Answer { .. } => {}
            Command::Shutdown => break,
            // Nothing runs between turns.
            Command::Interrupt | Command::Respond { .. } | Command::SetHandHolding(_) | Command::ReadTask { .. } | Command::StopTask { .. } => {}
        }
    }
    Ok(())
}

/// OpenAI-style providers known to take `stream_options.include_usage` (a final chunk with the
/// request's usage; Google's OpenAI-compatible endpoint documents it too). Others aren't sent it
/// (Mistral turns down fields it doesn't know); usage they send anyway is still counted.
const REPORTS_USAGE: &[&str] = &["openai", "openrouter", "deepseek", "xai", "groq", "google"];

/// Take in what an Anthropic stream event says about usage: `message_start` has the input
/// side (and output so far), `message_delta` the running output count. `long_writes`: how many
/// of the cache writes were 1-hour ones (`cache_creation`), priced apart.
fn anthropic_usage(v: &Value, usage: &mut TokenUsage, long_writes: &mut u64) {
    let u = match v["type"].as_str() {
        Some("message_start") => &v["message"]["usage"],
        Some("message_delta") => &v["usage"],
        _ => return,
    };
    let set = |n: &Value, field: &mut u64| {
        if let Some(n) = n.as_u64() {
            *field = n;
        }
    };
    set(&u["input_tokens"], &mut usage.input);
    set(&u["output_tokens"], &mut usage.output);
    set(&u["cache_read_input_tokens"], &mut usage.cache_read);
    set(&u["cache_creation_input_tokens"], &mut usage.cache_write);
    set(&u["cache_creation"]["ephemeral_1h_input_tokens"], long_writes);
}

/// An OpenAI-style chunk's `usage` (the last chunk, when asked for), and its cost when the
/// provider says (OpenRouter's `cost`). Prompt tokens include cached ones and cache writes,
/// which are counted apart here; output is all the model wrote, reasoning included (providers
/// that leave reasoning out of `completion_tokens` still count it in `total_tokens`).
fn openai_usage(v: &Value) -> Option<(TokenUsage, Option<f64>)> {
    let u = v.get("usage").filter(|u| u.is_object())?;
    let prompt = u["prompt_tokens"].as_u64().unwrap_or(0);
    let details = &u["prompt_tokens_details"];
    let cached = details["cached_tokens"].as_u64().unwrap_or(0).min(prompt);
    let written = details["cache_write_tokens"].as_u64().unwrap_or(0).min(prompt - cached);
    let completion = u["completion_tokens"].as_u64().unwrap_or(0);
    let output = completion.max(u["total_tokens"].as_u64().unwrap_or(0).saturating_sub(prompt));
    let t = TokenUsage { input: prompt - cached - written, output, cache_read: cached, cache_write: written };
    (!t.is_empty()).then_some((t, u["cost"].as_f64()))
}

/// A user message for `wire`. Without images the content is a plain string; with images it's
/// Anthropic image blocks before the text, or OpenAI `image_url` data URLs after it.
/// Unreadable images are skipped and reported.
fn user_message(wire: Wire, text: &str, images: &[PathBuf]) -> (Value, Vec<String>) {
    let mut errors = Vec::new();
    let mut loaded = Vec::new();
    for path in images {
        match load_image(path) {
            Ok(img) => loaded.push(img),
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    if loaded.is_empty() {
        return (json!({ "role": "user", "content": text }), errors);
    }
    let content: Vec<Value> = match wire {
        Wire::Anthropic => loaded
            .into_iter()
            .map(|(media_type, data)| json!({ "type": "image", "source": { "type": "base64", "media_type": media_type, "data": data } }))
            .chain([json!({ "type": "text", "text": text })])
            .collect(),
        _ => [json!({ "type": "text", "text": text })]
            .into_iter()
            .chain(loaded.into_iter().map(|(media_type, data)| {
                json!({ "type": "image_url", "image_url": { "url": format!("data:{media_type};base64,{data}") } })
            }))
            .collect(),
    };
    (json!({ "role": "user", "content": content }), errors)
}

/// Split an SSE byte stream into `data:` payloads.
fn sse_events(buf: &mut Vec<u8>) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
        let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| (i, 4));
        let Some((pos, end)) = lf.into_iter().chain(crlf).min_by_key(|(i, _)| *i) else { break };
        let block: Vec<u8> = buf.drain(..pos + end).collect();
        let text = String::from_utf8_lossy(&block[..pos]);
        let data = text
            .lines()
            .filter_map(|line| line.strip_prefix("data:").map(|s| s.strip_prefix(' ').unwrap_or(s)))
            .collect::<Vec<_>>()
            .join("\n");
        if !data.is_empty() {
            out.push(data);
        }
    }
    out
}

fn keep_command(cmd: Result<Command, async_channel::RecvError>, kept: &mut VecDeque<Command>, shutdown: &mut bool) -> Result<()> {
    match cmd {
        Ok(Command::Interrupt) => bail!("Interrupted"),
        Ok(Command::Shutdown) | Err(_) => {
            *shutdown = true;
            bail!("Interrupted")
        }
        Ok(cmd) => {
            kept.push_back(cmd);
            Ok(())
        }
    }
}

async fn send_request(
    req: reqwest::RequestBuilder,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
    timeout: Duration,
) -> Result<reqwest::Response> {
    let send = req.send();
    tokio::pin!(send);
    let sleep = tokio::time::sleep(timeout);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            response = &mut send => return Ok(response?),
            _ = &mut sleep => bail!("Timed out waiting for response headers after {} seconds", timeout.as_secs()),
            cmd = commands.recv() => keep_command(cmd, kept, shutdown)?,
        }
    }
}

fn anthropic_body(model: &str, effort: Effort, history: &[Value]) -> Value {
    let mut body = json!({
        "model": model,
        "max_tokens": 64000,
        "stream": true,
        "system": SYSTEM,
        "messages": history,
    });
    if model.starts_with("claude-haiku-4") {
        // Haiku 4.5 still uses a token budget for thinking.
        if effort >= Effort::Low {
            let budget = match effort {
                Effort::Low => 2048,
                Effort::Medium => 8192,
                _ => 16000,
            };
            body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
        }
    } else {
        // Current models: adaptive thinking, depth controlled by effort. Summaries make
        // reasoning readable in the transcript instead of a silent pause.
        body["thinking"] = json!({ "type": "adaptive", "display": "summarized" });
        let e = effort.clamp_to(&[Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max]);
        body["output_config"] = json!({ "effort": e.as_str() });
        // Opus/Fable/Sonnet 5.5: reroute safety-classifier refusals to a fallback model server-side.
        body["fallbacks"] = json!("default");
    }
    body
}

async fn anthropic_turn(
    client: &reqwest::Client,
    url: &str,
    key: &str,
    model: &str,
    effort: Effort,
    history: &[Value],
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<Value> {
    let body = anthropic_body(model, effort, history);
    let mut req = client
        .post(url)
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .json(&body);
    if body.get("fallbacks").is_some() {
        req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
    }
    let resp = send_request(req, commands, kept, shutdown, Duration::from_secs(60)).await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(String::from))
            .unwrap_or(text);
        return Err(request_error(status, &headers, format!("Anthropic API {status}: {msg}")));
    }
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    // Rebuild the assistant content blocks so they can be replayed unchanged next turn.
    let mut blocks: Vec<Value> = Vec::new();
    let mut text = String::new();
    let mut stop_reason = None;
    let mut terminal = false;
    let mut usage = TokenUsage::default();
    let mut long_writes = 0;
    loop {
        tokio::select! {
            chunk = stream.next() => {
                let Some(chunk) = chunk else { break };
                buf.extend_from_slice(&chunk?);
                for data in sse_events(&mut buf) {
                    let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
                    anthropic_usage(&v, &mut usage, &mut long_writes);
                    match v["type"].as_str() {
                        Some("content_block_start") => blocks.push(v["content_block"].clone()),
                        Some("content_block_delta") => {
                            let i = v["index"].as_u64().unwrap_or(0) as usize;
                            let d = &v["delta"];
                            match d["type"].as_str() {
                                Some("text_delta") => {
                                    let t = d["text"].as_str().unwrap_or_default();
                                    text.push_str(t);
                                    append(&mut blocks, i, "text", t);
                                    events.send(AgentEvent::TextDelta(t.into())).await?;
                                }
                                Some("thinking_delta") => {
                                    let t = d["thinking"].as_str().unwrap_or_default();
                                    append(&mut blocks, i, "thinking", t);
                                    events.send(AgentEvent::ReasoningDelta(t.into())).await?;
                                }
                                Some("signature_delta") => append(&mut blocks, i, "signature", d["signature"].as_str().unwrap_or_default()),
                                _ => {}
                            }
                        }
                        Some("message_delta") => stop_reason = v["delta"]["stop_reason"].as_str().map(String::from),
                        Some("message_stop") => terminal = true,
                        Some("error") => bail!("{}", v["error"]["message"].as_str().unwrap_or("stream error")),
                        _ => {}
                    }
                }
            }
            cmd = commands.recv() => keep_command(cmd, kept, shutdown)?,
        }
    }
    if !terminal {
        bail!("The connection closed before the answer finished");
    }
    if !usage.is_empty() {
        let cost = trek_core::pricing::request(model, &AgentId::Direct("anthropic".into()), &usage, long_writes, false);
        events.send(AgentEvent::Usage { model: Some(model.to_string()), tokens: usage, cost }).await?;
    }
    if stop_reason.as_deref() == Some("refusal") {
        bail!("The model declined this request.");
    }
    events.send(AgentEvent::TextDone(text)).await?;
    Ok(json!({ "role": "assistant", "content": blocks }))
}

fn append(blocks: &mut [Value], index: usize, field: &str, s: &str) {
    if let Some(Value::String(text)) = blocks.get_mut(index).and_then(|b| b.get_mut(field)) {
        text.push_str(s);
    }
}

#[allow(clippy::too_many_arguments)]
async fn openai_turn(
    client: &reqwest::Client,
    agent: &AgentId,
    base_url: &str,
    key: Option<&str>,
    local: bool,
    include_usage: bool,
    model: &str,
    effort: Effort,
    history: &[Value],
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<Value> {
    let mut messages = vec![json!({ "role": "system", "content": SYSTEM })];
    messages.extend(history.iter().cloned());
    let mut body = json!({ "model": model, "messages": messages, "stream": true });
    if include_usage {
        body["stream_options"] = json!({ "include_usage": true });
    }
    if !local && effort != Effort::Off {
        let e = effort.clamp_to(&[Effort::Low, Effort::Medium, Effort::High]);
        body["reasoning_effort"] = json!(e.as_str());
    }
    let mut req = client.post(format!("{base_url}/chat/completions")).json(&body);
    if let Some(k) = key {
        req = req.bearer_auth(k);
    }
    let resp = send_request(req, commands, kept, shutdown, Duration::from_secs(60)).await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().map(String::from)).unwrap_or(text);
        return Err(request_error(status, &headers, format!("{status}: {msg}")));
    }
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    let mut text = String::new();
    let mut usage = None;
    let mut terminal = false;
    loop {
        tokio::select! {
            chunk = stream.next() => {
                let Some(chunk) = chunk else { break };
                buf.extend_from_slice(&chunk?);
                for data in sse_events(&mut buf) {
                    if data == "[DONE]" {
                        terminal = true;
                        continue;
                    }
                    let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
                    usage = openai_usage(&v).or(usage);
                    terminal |= v["choices"].as_array().into_iter().flatten().any(|c| !c["finish_reason"].is_null());
                    let d = &v["choices"][0]["delta"];
                    if let Some(r) = d["reasoning_content"].as_str().or(d["reasoning"].as_str()) {
                        events.send(AgentEvent::ReasoningDelta(r.into())).await?;
                    }
                    if let Some(t) = d["content"].as_str() {
                        text.push_str(t);
                        events.send(AgentEvent::TextDelta(t.into())).await?;
                    }
                }
            }
            cmd = commands.recv() => keep_command(cmd, kept, shutdown)?,
        }
    }
    if !terminal {
        bail!("The connection closed before the answer finished");
    }
    if let Some((tokens, reported)) = usage {
        // One request: its prompt's size sets its tier (Gemini's and xAI's long context).
        let cost = match reported {
            Some(usd) => Some(UsageCost::reported(usd)),
            None => trek_core::pricing::request(model, agent, &tokens, 0, false),
        };
        events.send(AgentEvent::Usage { model: Some(model.to_string()), tokens, cost }).await?;
    }
    events.send(AgentEvent::TextDone(text.clone())).await?;
    Ok(json!({ "role": "assistant", "content": text }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read the request head, as a server does before it answers: a socket closed with the request
    /// unread resets the connection on Windows, and the client sees that instead of the answer.
    async fn read_request_head(socket: &mut tokio::net::TcpStream) {
        use tokio::io::AsyncReadExt as _;
        let mut head = vec![];
        let mut buf = [0u8; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match socket.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(&buf[..n]),
            }
        }
    }

    async fn sse_server(body: &'static str) -> String {
        use tokio::io::AsyncWriteExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request_head(&mut socket).await;
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
            let _ = socket.shutdown().await;
        });
        url
    }

    #[test]
    fn a_429_is_a_limit_and_other_failures_are_not() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "30".parse().unwrap());
        let e = request_error(reqwest::StatusCode::TOO_MANY_REQUESTS, &headers, "429 Too Many Requests: Rate limit exceeded".into());
        let Some(crate::limits::LimitError(limit)) = e.downcast_ref() else { panic!("{e:#}") };
        assert!(limit.resets_at.is_some_and(|at| (at - trek_core::store::now_ms() - 30_000).abs() < 5_000));
        assert_eq!(format!("{e:#}"), "429 Too Many Requests: Rate limit exceeded");
        let e = request_error(reqwest::StatusCode::BAD_REQUEST, &headers, "400 Bad Request: no such model".into());
        assert!(e.downcast_ref::<crate::limits::LimitError>().is_none());
    }

    #[test]
    fn sse_splits_complete_events_with_either_line_ending() {
        let mut buf = b"data: {\"a\":1}\n\ndata: [DONE]\r\n\r\ndata: {\"b\"".to_vec();
        assert_eq!(sse_events(&mut buf), vec!["{\"a\":1}".to_string(), "[DONE]".to_string()]);
        assert_eq!(buf, b"data: {\"b\"");
        buf.extend_from_slice(b":\"\xc3");
        assert!(sse_events(&mut buf).is_empty());
        buf.extend_from_slice(b"\xa9\"}\r\ndata: second line\r\n\r\n");
        assert_eq!(sse_events(&mut buf), vec!["{\"b\":\"é\"}\nsecond line".to_string()]);
    }

    #[test]
    fn appending_deltas_updates_the_existing_string() {
        let mut blocks = vec![json!({"type":"text","text":"one"})];
        append(&mut blocks, 0, "text", " two");
        append(&mut blocks, 0, "text", " three");
        assert_eq!(blocks[0]["text"], "one two three");
    }

    #[test]
    fn streaming_keeps_non_control_commands_in_order() {
        let mut kept = VecDeque::new();
        let mut shutdown = false;
        for command in [
            Command::SetHandHolding(trek_core::HandHolding::Supervised),
            Command::SetModel { model: "next".into(), effort: Effort::Medium },
            Command::SetModes { plan: true, fast: Some("fast".into()), effort: Effort::High },
            Command::Prompt { text: "steer".into(), images: vec![] },
        ] {
            keep_command(Ok(command), &mut kept, &mut shutdown).unwrap();
        }
        assert!(matches!(kept.pop_front(), Some(Command::SetHandHolding(trek_core::HandHolding::Supervised))));
        assert!(matches!(kept.pop_front(), Some(Command::SetModel { model, .. }) if model == "next"));
        assert!(matches!(kept.pop_front(), Some(Command::SetModes { effort: Effort::High, .. })));
        assert!(matches!(kept.pop_front(), Some(Command::Prompt { text, .. }) if text == "steer"));
        assert!(!shutdown && kept.is_empty());
    }

    #[tokio::test]
    async fn response_headers_keep_commands_and_time_out() {
        use tokio::io::AsyncWriteExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request_head(&mut socket).await;
            tokio::time::sleep(Duration::from_millis(30)).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await.unwrap();
        });
        let (tx, rx) = async_channel::unbounded();
        tx.send(Command::SetModes { plan: true, fast: None, effort: Effort::High }).await.unwrap();
        tx.send(Command::Prompt { text: "steer".into(), images: vec![] }).await.unwrap();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        send_request(reqwest::Client::new().get(&url), &rx, &mut kept, &mut shutdown, Duration::from_secs(1)).await.unwrap();
        assert!(matches!(kept.pop_front(), Some(Command::SetModes { effort: Effort::High, .. })));
        assert!(matches!(kept.pop_front(), Some(Command::Prompt { text, .. }) if text == "steer"));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let e = send_request(reqwest::Client::new().get(&url), &rx, &mut kept, &mut shutdown, Duration::from_millis(20)).await.unwrap_err();
        assert!(e.to_string().contains("Timed out waiting for response headers"), "{e:#}");
    }

    #[tokio::test]
    async fn openai_stream_requires_a_terminal_marker() {
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, _rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let complete = sse_server("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n").await;
        let answer = openai_turn(&client, &AgentId::Direct("local".into()), &complete, None, true, false, "m", Effort::Off, &[], &events, &commands, &mut kept, &mut shutdown).await.unwrap();
        assert_eq!(answer["content"], "ok");

        let incomplete = sse_server("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n").await;
        let e = openai_turn(&client, &AgentId::Direct("local".into()), &incomplete, None, true, false, "m", Effort::Off, &[], &events, &commands, &mut kept, &mut shutdown).await.unwrap_err();
        assert!(e.to_string().contains("connection closed before the answer finished"), "{e:#}");
    }

    #[tokio::test]
    async fn anthropic_stream_requires_message_stop() {
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, _rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let complete = sse_server(
            "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n",
        )
        .await;
        let answer = anthropic_turn(&client, &complete, "key", "m", Effort::Off, &[], &events, &commands, &mut kept, &mut shutdown).await.unwrap();
        assert_eq!(answer["content"][0]["text"], "ok");

        let incomplete = sse_server(
            "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
        )
        .await;
        let e = anthropic_turn(&client, &incomplete, "key", "m", Effort::Off, &[], &events, &commands, &mut kept, &mut shutdown).await.unwrap_err();
        assert!(e.to_string().contains("connection closed before the answer finished"), "{e:#}");
    }

    #[test]
    fn user_message_attaches_images_per_wire() {
        let dir = std::env::temp_dir().join(format!("trek-direct-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let jpg = dir.join("a.JPG");
        std::fs::write(&jpg, b"abc").unwrap();
        let (plain, _) = user_message(Wire::Anthropic, "hi", &[]);
        assert_eq!(plain, json!({"role":"user","content":"hi"}));
        let (a, errs) = user_message(Wire::Anthropic, "hi", &[jpg.clone()]);
        assert!(errs.is_empty());
        assert_eq!(a["content"][0], json!({"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"YWJj"}}));
        assert_eq!(a["content"][1], json!({"type":"text","text":"hi"}));
        let (o, _) = user_message(Wire::OpenAiChat, "hi", &[jpg]);
        assert_eq!(o["content"][1], json!({"type":"image_url","image_url":{"url":"data:image/jpeg;base64,YWJj"}}));
        let (missing, errs) = user_message(Wire::OpenAiChat, "hi", &[dir.join("nope.png")]);
        assert_eq!((missing["content"].as_str(), errs.len()), (Some("hi"), 1));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn usage_from_both_wires() {
        // Anthropic's stream: the input side up front, the output count as it ends.
        let (mut u, mut long) = (TokenUsage::default(), 0);
        anthropic_usage(&json!({"type":"message_start","message":{"id":"m","usage":{"input_tokens":12,"cache_creation_input_tokens":800,"cache_read_input_tokens":4000,"output_tokens":1,
            "cache_creation":{"ephemeral_5m_input_tokens":300,"ephemeral_1h_input_tokens":500}}}}), &mut u, &mut long);
        anthropic_usage(&json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}), &mut u, &mut long);
        anthropic_usage(&json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":57}}), &mut u, &mut long);
        assert_eq!((u, long), (TokenUsage { input: 12, output: 57, cache_read: 4000, cache_write: 800 }, 500));
        // On Opus 5.5: 12 × $4 + 57 × $20 + 4,000 × $0.20 + 300 × $5 + 500 × $8, per million.
        let cost = trek_core::pricing::request("claude-opus-5-5", &AgentId::Direct("anthropic".into()), &u, long, false).unwrap();
        assert!((cost.usd - (0.000048 + 0.00114 + 0.0008 + 0.0015 + 0.004)).abs() < 1e-12);
        // OpenAI's last chunk, asked for with include_usage: cached prompt tokens counted apart.
        let last = json!({"id":"c","object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":1200,"completion_tokens":80,"total_tokens":1280,"prompt_tokens_details":{"cached_tokens":1024}}});
        assert_eq!(openai_usage(&last), Some((TokenUsage { input: 176, output: 80, cache_read: 1024, cache_write: 0 }, None)));
        // Cache writes (GPT-5.6 and later) are part of the prompt too; OpenRouter adds its cost.
        let written = json!({"usage":{"prompt_tokens":5000,"completion_tokens":10,"total_tokens":5010,"prompt_tokens_details":{"cached_tokens":1000,"cache_write_tokens":3000},"cost":0.0042}});
        assert_eq!(openai_usage(&written), Some((TokenUsage { input: 1000, output: 10, cache_read: 1000, cache_write: 3000 }, Some(0.0042))));
        // Reasoning counted outside `completion_tokens` (Gemini's thoughts) still counts, through
        // the total.
        let thoughts = json!({"usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":180}});
        assert_eq!(openai_usage(&thoughts).map(|(t, _)| t.output), Some(80));
        assert!(REPORTS_USAGE.contains(&"google"), "Gemini is asked for its usage");
        assert_eq!(openai_usage(&json!({"choices":[{"delta":{"content":"x"}}],"usage":null})), None);
    }

    #[test]
    fn anthropic_body_uses_adaptive_thinking_and_effort() {
        let b = anthropic_body("claude-opus-5-5", Effort::XHigh, &[]);
        assert_eq!(b["thinking"]["type"], "adaptive");
        assert_eq!(b["output_config"]["effort"], "xhigh");
        assert!(b.get("budget_tokens").is_none());
        let h = anthropic_body("claude-haiku-4-5", Effort::Medium, &[]);
        assert_eq!(h["thinking"]["budget_tokens"], 8192);
        assert!(h.get("output_config").is_none());
    }
}
