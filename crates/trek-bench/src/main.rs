//! `trek-bench`: Trek's own agent loop (the Direct harness, `trek-agents`' `direct.rs`) run
//! headless on one prompt, for benchmarks (Terminal-Bench through a Harbor adapter). It runs the
//! prompt with Full access, reports each model request and tool call as it happens, and ends
//! with a `done` object of totals: requests, tool calls, tokens, cost and time.
//!
//! Everything on stdout is protocol, one JSON object per line; human logs go to stderr.
//!
//! - events: `request_start {n}`, `request_end {n, usage, cost_usd, elapsed_ms, error?}`,
//!   `text {text}`, `tool_call {name, args}`, `tool_result {name, chars, ok}`,
//!   `script_result {nested_calls, calls_by_tool, printed_bytes, truncated_bytes, elapsed_ms,
//!   timed_out}`
//! - with `--executor stdio`, tool requests too (`exec`, `read`, `write`, each with an `id`),
//!   answered on stdin; see `StdioExecutor` in `trek-agents` for their shapes
//! - last, `done` (also written to `--out`), then the process exits: 0 when the model ended its
//!   turn, 3 when the turn cap or the time limit ended it, 2 on an error.
//!
//! Builds wherever `trek-agents` does; the stdio executor is meant for a Linux sandbox driven
//! from macOS or Linux.

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt as _;
use trek_agents::direct_harness::{
    DirectOptions, LocalExecutor, Observer, ScriptResult, StdioExecutor, Stop, ToolExecutor,
    endpoint, run_with,
};
use trek_agents::{AgentEvent, Command, SessionConfig};
use trek_core::catalog::{DirectProvider, Wire, direct_provider};
use trek_core::{AgentId, Effort, HandHolding, TokenUsage, UsageCost};

const USAGE: &str = "\
usage: trek-bench --prompt-file P --cwd DIR --model MODEL [options]

  --provider ID            a Direct provider (default openrouter), or any name with --wire
  --wire openai|anthropic  the wire for a provider Trek doesn't know (needs --base-url)
  --base-url URL           the provider's endpoint instead of its own (e.g. a LiteLLM proxy)
  --effort LEVEL           off|minimal|low|medium|high|xhigh|max (default high)
  --executor local|stdio   run tools here, or ask a peer over stdout/stdin (default local)
  --script on|off          offer the script tool (default off)
  --max-turns N            model requests for the prompt (default 200)
  --time-limit-s S         no new request or tool call after this (default 3600)
  --command-timeout-s S    one bash command's limit (default 600)
  --api-key-file F         the key (else the provider's env var, else TREK_BENCH_API_KEY)
  --out FILE               also write the done object here
  --instructions-file F    extra system instructions, after the system prompt
  --system-prompt-file F   the whole system prompt, in place of Trek's
  --script-log-dir DIR     where script runs log their calls (default: script-logs by --out)";

#[derive(Debug)]
struct Args {
    prompt_file: PathBuf,
    cwd: PathBuf,
    model: String,
    provider: String,
    wire: Option<Wire>,
    base_url: Option<String>,
    effort: Effort,
    stdio: bool,
    script: bool,
    max_turns: u32,
    time_limit: Duration,
    command_timeout: Duration,
    api_key_file: Option<PathBuf>,
    out: Option<PathBuf>,
    instructions_file: Option<PathBuf>,
    system_prompt_file: Option<PathBuf>,
    script_log_dir: Option<PathBuf>,
}

fn parse_args(mut argv: impl Iterator<Item = String>) -> Result<Args> {
    let mut a = Args {
        prompt_file: PathBuf::new(),
        cwd: PathBuf::new(),
        model: String::new(),
        provider: "openrouter".into(),
        wire: None,
        base_url: None,
        effort: Effort::High,
        stdio: false,
        script: false,
        max_turns: 200,
        time_limit: Duration::from_secs(3600),
        command_timeout: Duration::from_secs(600),
        api_key_file: None,
        out: None,
        instructions_file: None,
        system_prompt_file: None,
        script_log_dir: None,
    };
    let secs = |v: String, flag: &str| -> Result<Duration> {
        let s: f64 = v.parse().with_context(|| format!("{flag} takes seconds"))?;
        if s.is_nan() || s <= 0.0 || !s.is_finite() {
            bail!("{flag} must be more than 0");
        }
        Ok(Duration::from_secs_f64(s))
    };
    while let Some(flag) = argv.next() {
        if flag == "-h" || flag == "--help" {
            bail!("{USAGE}");
        }
        let value = argv
            .next()
            .with_context(|| format!("{flag} needs a value\n\n{USAGE}"))?;
        match flag.as_str() {
            "--prompt-file" => a.prompt_file = value.into(),
            "--cwd" => a.cwd = value.into(),
            "--model" => a.model = value,
            "--provider" => a.provider = value,
            "--wire" => {
                a.wire = Some(match value.as_str() {
                    "openai" => Wire::OpenAiChat,
                    "anthropic" => Wire::Anthropic,
                    _ => bail!("--wire is openai or anthropic"),
                })
            }
            "--base-url" => a.base_url = Some(value),
            "--effort" => {
                a.effort = Effort::parse(&value).with_context(|| format!("no effort {value}"))?
            }
            "--executor" => {
                a.stdio = match value.as_str() {
                    "local" => false,
                    "stdio" => true,
                    _ => bail!("--executor is local or stdio"),
                }
            }
            "--script" => {
                a.script = match value.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => bail!("--script is on or off"),
                }
            }
            "--max-turns" => {
                a.max_turns = value.parse().context("--max-turns takes a number")?;
                if a.max_turns == 0 {
                    bail!("--max-turns must be at least 1");
                }
            }
            "--time-limit-s" => a.time_limit = secs(value, &flag)?,
            "--command-timeout-s" => a.command_timeout = secs(value, &flag)?,
            "--api-key-file" => a.api_key_file = Some(value.into()),
            "--out" => a.out = Some(value.into()),
            "--instructions-file" => a.instructions_file = Some(value.into()),
            "--system-prompt-file" => a.system_prompt_file = Some(value.into()),
            "--script-log-dir" => a.script_log_dir = Some(value.into()),
            _ => bail!("unknown option {flag}\n\n{USAGE}"),
        }
    }
    for (missing, flag) in [
        (a.prompt_file.as_os_str().is_empty(), "--prompt-file"),
        (a.cwd.as_os_str().is_empty(), "--cwd"),
        (a.model.is_empty(), "--model"),
    ] {
        if missing {
            bail!("{flag} is required\n\n{USAGE}");
        }
    }
    Ok(a)
}

fn read_file(path: &PathBuf, what: &str) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading the {what} {}", path.display()))
}

fn usage_json(u: &TokenUsage) -> Value {
    json!({
        "input": u.input,
        "cache_read": u.cache_read,
        "cache_write": u.cache_write,
        "output": u.output,
    })
}

/// What the run added up to, for `done`.
#[derive(Default)]
struct Totals {
    requests: u32,
    /// Requests that came back with an answer.
    turns: u32,
    tool_calls: u32,
    script_calls: u32,
    nested_calls: u64,
    usage: TokenUsage,
    cost: f64,
    reported: u32,
    priced: u32,
    /// Answered requests no cost is known for.
    unpriced: u32,
    final_text: String,
    stopped: Option<Stop>,
}

/// The session's observer: each event a line on stdout, and the totals kept.
struct Bench {
    out: tokio::sync::mpsc::UnboundedSender<String>,
    totals: Mutex<Totals>,
}

impl Bench {
    fn emit(&self, v: Value) {
        let _ = self.out.send(v.to_string());
    }
}

impl Observer for Bench {
    fn request_start(&self, n: u32) {
        self.totals.lock().unwrap().requests = n;
        self.emit(json!({ "type": "request_start", "n": n }));
    }

    fn request_end(
        &self,
        n: u32,
        usage: Option<&TokenUsage>,
        cost: Option<UsageCost>,
        elapsed: Duration,
        error: Option<&str>,
    ) {
        {
            let mut t = self.totals.lock().unwrap();
            if error.is_none() {
                t.turns += 1;
            }
            if let Some(u) = usage {
                t.usage.input += u.input;
                t.usage.output += u.output;
                t.usage.cache_read += u.cache_read;
                t.usage.cache_write += u.cache_write;
            }
            match cost {
                Some(c) => {
                    t.cost += c.usd;
                    if c.reported {
                        t.reported += 1;
                    } else {
                        t.priced += 1;
                    }
                }
                None if error.is_none() => t.unpriced += 1,
                None => {}
            }
        }
        let mut v = json!({
            "type": "request_end",
            "n": n,
            "usage": usage.map(usage_json),
            "cost_usd": cost.map(|c| c.usd),
            "cost_reported": cost.map(|c| c.reported),
            "elapsed_ms": elapsed.as_millis() as u64,
        });
        if let Some(e) = error {
            v["error"] = json!(e);
        }
        self.emit(v);
    }

    fn text(&self, text: &str) {
        self.totals.lock().unwrap().final_text = text.to_string();
        self.emit(json!({ "type": "text", "text": text }));
    }

    fn tool_call(&self, name: &str, args: &Value) {
        {
            let mut t = self.totals.lock().unwrap();
            t.tool_calls += 1;
            if name == "script" {
                t.script_calls += 1;
            }
        }
        self.emit(json!({ "type": "tool_call", "name": name, "args": args }));
    }

    fn tool_result(&self, name: &str, output: &str, ok: bool) {
        self.emit(json!({
            "type": "tool_result",
            "name": name,
            "chars": output.chars().count(),
            "ok": ok,
        }));
    }

    fn script_result(&self, r: &ScriptResult) {
        self.totals.lock().unwrap().nested_calls += r.nested_calls as u64;
        self.emit(json!({
            "type": "script_result",
            "nested_calls": r.nested_calls,
            "calls_by_tool": r.calls_by_tool,
            "printed_bytes": r.printed.len(),
            "truncated_bytes": r.truncated_bytes,
            "elapsed_ms": r.elapsed_ms,
            "timed_out": r.timed_out,
            "error": r.error,
            "log_path": r.log_path,
        }));
    }

    fn stopped(&self, why: &Stop) {
        self.totals.lock().unwrap().stopped = Some(why.clone());
    }
}

/// What the run was, for `done` (filled in as far as it got).
#[derive(Default)]
struct Setup {
    model: String,
    provider: String,
    base_url: Option<String>,
    executor: &'static str,
    script: bool,
}

/// Runs the prompt. Returns whether the bench had to interrupt the turn past the time limit,
/// and the turn's error if it had one (why it stopped otherwise is the observer's).
async fn bench(
    args: Args,
    out: tokio::sync::mpsc::UnboundedSender<String>,
    observer: Arc<Bench>,
    setup: &mut Setup,
) -> Result<(bool, Option<String>)> {
    let started = Instant::now();
    setup.model = args.model.clone();
    setup.provider = args.provider.clone();
    setup.executor = if args.stdio { "stdio" } else { "local" };
    setup.script = args.script;

    let prompt = read_file(&args.prompt_file, "prompt")?;
    let instructions = match &args.instructions_file {
        Some(f) => Some(read_file(f, "instructions")?),
        None => None,
    };
    let system_prompt = match &args.system_prompt_file {
        Some(f) => Some(read_file(f, "system prompt")?),
        None => None,
    };
    if trek_core::catalog::is_mock(&args.provider) {
        bail!("the mock agent isn't the Direct harness; pick a real provider");
    }
    let known = direct_provider(&args.provider).copied();
    let env_key = known.and_then(|p| p.env_key);
    let key = match &args.api_key_file {
        Some(f) => Some(read_file(f, "API key file")?.trim().to_string()),
        None => env_key
            .and_then(|k| std::env::var(k).ok())
            .or_else(|| std::env::var("TREK_BENCH_API_KEY").ok()),
    }
    .filter(|k| !k.is_empty());
    let provider = match known {
        Some(p) => {
            if args.wire.is_some_and(|w| w != p.wire) {
                bail!("--wire doesn't match {}'s own", p.name);
            }
            if !p.local && key.is_none() {
                bail!(
                    "no API key for {}: pass --api-key-file or set {}",
                    p.name,
                    env_key.unwrap_or("TREK_BENCH_API_KEY")
                );
            }
            p
        }
        None => {
            let wire = args.wire.with_context(|| {
                format!(
                    "Trek doesn't know the provider {}: give --wire openai|anthropic and --base-url",
                    args.provider
                )
            })?;
            let base = args
                .base_url
                .clone()
                .context("a provider Trek doesn't know needs --base-url")?;
            let name: &'static str = args.provider.clone().leak();
            DirectProvider {
                id: name,
                name,
                base_url: base.leak(),
                wire,
                env_key: None,
                // Without a key it's taken for a local server (no reasoning effort is sent).
                local: key.is_none(),
            }
        }
    };
    setup.base_url = Some(endpoint(&provider, args.base_url.as_deref()));

    let executor: Arc<dyn ToolExecutor> = if args.stdio {
        Arc::new(StdioExecutor::new(out.clone(), tokio::io::stdin()))
    } else {
        Arc::new(LocalExecutor)
    };
    let script_log_dir = args.script_log_dir.clone().or_else(|| {
        let out = args.out.as_ref()?;
        Some(out.parent().unwrap_or(out.as_path()).join("script-logs"))
    });
    let deadline = started + args.time_limit;
    let config = SessionConfig {
        agent: AgentId::Direct(provider.id.to_string()),
        cwd: args.cwd.clone(),
        model: Some(args.model.clone()),
        effort: args.effort,
        hand_holding: HandHolding::FullAccess,
        plan: false,
        read_only: false,
        resume: None,
        resume_at: None,
        fork: false,
        recap: None,
        fast: None,
        mcp_servers: vec![],
        instructions,
        read_dirs: vec![],
        script: args.script,
    };
    let options = DirectOptions {
        api_key: Some(key),
        provider: Some(provider),
        base_url: args.base_url.clone(),
        executor,
        script_log_dir: args.script.then_some(script_log_dir).flatten(),
        max_turns: args.max_turns,
        command_timeout: Some(args.command_timeout),
        deadline: Some(deadline),
        system_prompt,
        observer: Some(observer),
    };

    let (commands, commands_rx) = async_channel::unbounded();
    let (events_tx, events) = async_channel::unbounded();
    let session = tokio::spawn(run_with(config, options, commands_rx, events_tx));
    commands
        .send(Command::Prompt {
            text: prompt,
            images: vec![],
        })
        .await?;

    // The limit is soft (the running request or tool finishes); past it by a command's own
    // limit and a minute more, the turn is stopped outright.
    let hard =
        tokio::time::Instant::from_std(deadline + args.command_timeout) + Duration::from_secs(60);
    let mut interrupted = false;
    let ended = loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(AgentEvent::TurnComplete { error }) => break Ok(error),
                Ok(AgentEvent::Notice(n)) => eprintln!("trek-bench: {n}"),
                Ok(AgentEvent::Error(e)) => eprintln!("trek-bench: error: {e}"),
                Ok(AgentEvent::LimitReached { message, .. }) => eprintln!("trek-bench: {message}"),
                Ok(_) => {}
                // The session ended before the turn did: it failed to start.
                Err(_) => break Err(()),
            },
            _ = tokio::time::sleep_until(hard), if !interrupted => {
                eprintln!("trek-bench: past the time limit; stopping the turn");
                interrupted = true;
                let _ = commands.send(Command::Interrupt).await;
            }
            _ = tokio::time::sleep_until(hard + Duration::from_secs(30)), if interrupted => {
                break Ok(Some("the turn didn't stop after the time limit".into()));
            }
        }
    };
    let _ = commands.send(Command::Shutdown).await;
    let error = match ended {
        Ok(error) => error,
        Err(()) => match tokio::time::timeout(Duration::from_secs(10), session).await {
            Ok(Ok(Err(e))) => Some(format!("{e:#}")),
            Ok(Err(e)) => Some(format!("the session panicked: {e}")),
            _ => Some("the session ended without finishing the turn".into()),
        },
    };
    Ok((interrupted, error))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let started = Instant::now();
    let (out, mut lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    // One writer, so protocol lines never interleave. An empty line ends it (sent after `done`;
    // nothing else sends one).
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(line) = lines.recv().await {
            if line.is_empty() {
                break;
            }
            let _ = stdout.write_all(line.as_bytes()).await;
            let _ = stdout.write_all(b"\n").await;
            let _ = stdout.flush().await;
        }
    });
    let observer = Arc::new(Bench {
        out: out.clone(),
        totals: Mutex::new(Totals::default()),
    });
    let mut setup = Setup::default();
    let parsed = parse_args(std::env::args().skip(1));
    let out_file = parsed.as_ref().ok().and_then(|a| a.out.clone());
    let (stopped, error) = match parsed {
        Ok(args) => match bench(args, out.clone(), observer.clone(), &mut setup).await {
            Ok((interrupted, error)) => {
                let t = observer.totals.lock().unwrap();
                let stopped = match (&t.stopped, interrupted) {
                    (_, true) => "time_limit",
                    (Some(Stop::EndTurn), _) => "end_turn",
                    (Some(Stop::MaxTurns), _) => "max_turns",
                    (Some(Stop::TimeLimit), _) => "time_limit",
                    (Some(Stop::Error(_)), _) | (None, _) => "error",
                };
                let error = match &t.stopped {
                    Some(Stop::Error(e)) => Some(e.clone()),
                    _ => error,
                };
                (stopped, if stopped == "error" { error } else { None })
            }
            Err(e) => ("error", Some(format!("{e:#}"))),
        },
        Err(e) => ("error", Some(format!("{e:#}"))),
    };
    if let Some(e) = &error {
        eprintln!("trek-bench: {e}");
    }

    // In a block of its own: the guard must be gone before the writer is awaited below.
    let done = {
        let t = observer.totals.lock().unwrap();
        let cost_source = match (t.reported, t.priced) {
            (0, 0) => Value::Null,
            (_, 0) => json!("reported"),
            (0, _) => json!("priced"),
            _ => json!("mixed"),
        };
        json!({
            "type": "done",
            "requests": t.requests,
            "tool_calls": t.tool_calls,
            "script_calls": t.script_calls,
            "nested_calls": t.nested_calls,
            "usage": usage_json(&t.usage),
            "cost_usd": (t.reported + t.priced > 0).then_some(t.cost),
            "cost_source": cost_source,
            "requests_without_cost": t.unpriced,
            "elapsed_s": started.elapsed().as_secs_f64(),
            "turns": t.turns,
            "stopped": stopped,
            "error": error,
            "final_text": t.final_text,
            "model": setup.model,
            "provider": setup.provider,
            "base_url": setup.base_url,
            "executor": setup.executor,
            "script": setup.script,
        })
    };
    if let Some(path) = &out_file {
        let written = serde_json::to_string_pretty(&done)
            .map_err(anyhow::Error::from)
            .and_then(|s| std::fs::write(path, s + "\n").map_err(Into::into));
        if let Err(e) = written {
            eprintln!("trek-bench: writing {}: {e:#}", path.display());
        }
    }
    let _ = out.send(done.to_string());
    let _ = out.send(String::new());
    let _ = tokio::time::timeout(Duration::from_secs(10), writer).await;
    std::process::exit(match stopped {
        "end_turn" => 0,
        "max_turns" | "time_limit" => 3,
        _ => 2,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Args> {
        parse_args(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn flags_parse_with_defaults() {
        let a = args(&["--prompt-file", "p", "--cwd", "/app", "--model", "m"]).unwrap();
        assert_eq!(a.provider, "openrouter");
        assert_eq!(a.effort, Effort::High);
        assert_eq!((a.stdio, a.script, a.max_turns), (false, false, 200));
        assert_eq!(a.time_limit, Duration::from_secs(3600));
        let a = args(&[
            "--prompt-file",
            "p",
            "--cwd",
            "/app",
            "--model",
            "m",
            "--provider",
            "litellm",
            "--wire",
            "openai",
            "--base-url",
            "http://localhost:4200/v1",
            "--executor",
            "stdio",
            "--script",
            "on",
            "--max-turns",
            "5",
            "--time-limit-s",
            "1.5",
            "--effort",
            "low",
        ])
        .unwrap();
        assert_eq!(a.wire, Some(Wire::OpenAiChat));
        assert_eq!((a.stdio, a.script, a.max_turns), (true, true, 5));
        assert_eq!(a.time_limit, Duration::from_millis(1500));
        assert_eq!(a.effort, Effort::Low);
        assert!(args(&["--cwd", "/app", "--model", "m"]).is_err());
        assert!(
            args(&[
                "--prompt-file",
                "p",
                "--cwd",
                "/app",
                "--model",
                "m",
                "--bogus",
                "1"
            ])
            .is_err()
        );
        assert!(
            args(&[
                "--prompt-file",
                "p",
                "--cwd",
                "/app",
                "--model",
                "m",
                "--max-turns"
            ])
            .is_err()
        );
    }
}
