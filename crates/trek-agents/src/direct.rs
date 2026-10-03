//! Direct providers (API keys and local servers). Phase 1 is streaming chat with full history;
//! Trek's own tool loop (read/edit/bash with the hand-holding gates) lands in phase 2.

use crate::{AgentEvent, Billing, Command, SessionConfig, load_image};
use std::path::PathBuf;
use anyhow::{Context as _, Result, bail};
use futures::StreamExt;
use serde_json::{Value, json};
use trek_core::catalog::{Wire, direct_provider};
use trek_core::settings::secrets;
use trek_core::{AgentId, Effort};

/// The error for a failed request: a usage limit for a 429 (said with `AgentEvent::LimitReached`),
/// else `message` as it is.
fn request_error(status: reqwest::StatusCode, headers: &reqwest::header::HeaderMap, message: String) -> anyhow::Error {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return crate::limits::LimitError(crate::limits::from_response(headers, message, trek_core::store::now_ms())).into();
    }
    anyhow::anyhow!(message)
}

const SYSTEM: &str = "You are Trek, a coding assistant. Be direct and concise. Use Markdown with fenced code blocks.";

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

    while let Ok(cmd) = commands.recv().await {
        match cmd {
            Command::Prompt { text, images } => {
                let (message, skipped) = user_message(provider.wire, &text, &images);
                for e in skipped {
                    events.send(AgentEvent::Notice(format!("Image left out: {e}"))).await?;
                }
                history.push(message);
                let result = match provider.wire {
                    Wire::Anthropic => anthropic_turn(&client, key.as_deref().unwrap_or_default(), &model, effort, &history, &events, &commands).await,
                    _ => openai_turn(&client, provider.base_url, key.as_deref(), provider.local, &model, effort, &history, &events, &commands).await,
                };
                match result {
                    Ok(assistant) => {
                        history.push(assistant);
                        events.send(AgentEvent::TurnComplete { cost_usd: None, error: None }).await?;
                    }
                    Err(e) => {
                        history.pop();
                        if let Some(crate::limits::LimitError(limit)) = e.downcast_ref() {
                            events.send(limit.clone().event()).await?;
                        }
                        events.send(AgentEvent::TurnComplete { cost_usd: None, error: Some(format!("{e:#}")) }).await?;
                    }
                }
            }
            Command::SetModel { model: m, effort: e } => {
                model = m;
                effort = e;
            }
            Command::Answer { .. } => {}
            Command::Shutdown => break,
            Command::Interrupt | Command::Respond { .. } | Command::SetHandHolding(_) => {}
        }
    }
    Ok(())
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
fn sse_events(buf: &mut String) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(pos) = buf.find("\n\n") {
        let block: String = buf.drain(..pos + 2).collect();
        for line in block.lines() {
            if let Some(data) = line.strip_prefix("data:") {
                out.push(data.trim().to_string());
            }
        }
    }
    out
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
    key: &str,
    model: &str,
    effort: Effort,
    history: &[Value],
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
) -> Result<Value> {
    let body = anthropic_body(model, effort, history);
    let mut req = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .json(&body);
    if body.get("fallbacks").is_some() {
        req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
    }
    let resp = req.send().await?;
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
    let mut buf = String::new();
    // Rebuild the assistant content blocks so they can be replayed unchanged next turn.
    let mut blocks: Vec<Value> = Vec::new();
    let mut text = String::new();
    let mut stop_reason = None;
    loop {
        tokio::select! {
            chunk = stream.next() => {
                let Some(chunk) = chunk else { break };
                buf.push_str(&String::from_utf8_lossy(&chunk?));
                for data in sse_events(&mut buf) {
                    let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
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
                        Some("error") => bail!("{}", v["error"]["message"].as_str().unwrap_or("stream error")),
                        _ => {}
                    }
                }
            }
            cmd = commands.recv() => {
                if matches!(cmd, Ok(Command::Interrupt) | Ok(Command::Shutdown) | Err(_)) {
                    bail!("Interrupted");
                }
            }
        }
    }
    if stop_reason.as_deref() == Some("refusal") {
        bail!("The model declined this request.");
    }
    events.send(AgentEvent::TextDone(text)).await?;
    Ok(json!({ "role": "assistant", "content": blocks }))
}

fn append(blocks: &mut [Value], index: usize, field: &str, s: &str) {
    if let Some(b) = blocks.get_mut(index) {
        let cur = b[field].as_str().unwrap_or_default().to_string();
        b[field] = json!(cur + s);
    }
}

#[allow(clippy::too_many_arguments)]
async fn openai_turn(
    client: &reqwest::Client,
    base_url: &str,
    key: Option<&str>,
    local: bool,
    model: &str,
    effort: Effort,
    history: &[Value],
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
) -> Result<Value> {
    let mut messages = vec![json!({ "role": "system", "content": SYSTEM })];
    messages.extend(history.iter().cloned());
    let mut body = json!({ "model": model, "messages": messages, "stream": true });
    if !local && effort != Effort::Off {
        let e = effort.clamp_to(&[Effort::Low, Effort::Medium, Effort::High]);
        body["reasoning_effort"] = json!(e.as_str());
    }
    let mut req = client.post(format!("{base_url}/chat/completions")).json(&body);
    if let Some(k) = key {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().map(String::from)).unwrap_or(text);
        return Err(request_error(status, &headers, format!("{status}: {msg}")));
    }
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut text = String::new();
    loop {
        tokio::select! {
            chunk = stream.next() => {
                let Some(chunk) = chunk else { break };
                buf.push_str(&String::from_utf8_lossy(&chunk?));
                for data in sse_events(&mut buf) {
                    if data == "[DONE]" { continue; }
                    let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
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
            cmd = commands.recv() => {
                if matches!(cmd, Ok(Command::Interrupt) | Ok(Command::Shutdown) | Err(_)) {
                    bail!("Interrupted");
                }
            }
        }
    }
    events.send(AgentEvent::TextDone(text.clone())).await?;
    Ok(json!({ "role": "assistant", "content": text }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn sse_splits_complete_events_only() {
        let mut buf = "data: {\"a\":1}\n\ndata: [DONE]\n\ndata: {\"b\"".to_string();
        assert_eq!(sse_events(&mut buf), vec!["{\"a\":1}".to_string(), "[DONE]".to_string()]);
        assert_eq!(buf, "data: {\"b\"");
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
