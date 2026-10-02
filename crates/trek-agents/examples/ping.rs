//! End-to-end: `cargo run -p trek-agents --example ping -- claude|codex|opencode`
use trek_agents::{AgentEvent, Command, SessionConfig, start};
use trek_core::{AgentId, Effort, HandHolding};

fn main() {
    let which = std::env::args().nth(1).unwrap_or("claude".into());
    let (agent, model) = match which.as_str() {
        "codex" => (AgentId::Codex, Some("gpt-5.6-luna")),
        "opencode" => (AgentId::OpenCode, None),
        _ => (AgentId::ClaudeCode, Some("claude-haiku-4-5")),
    };
    let h = start(SessionConfig {
        agent, cwd: "/tmp/trek-e2e".into(), model: model.map(String::from), effort: Effort::Low,
        hand_holding: HandHolding::Supervised, plan: false, resume: None, fast: None,
    });
    trek_core::runtime().block_on(async {
        h.commands.send(Command::Prompt("Reply with just the word: pong".into())).await.unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
        let mut deltas = 0;
        while let Ok(Ok(ev)) = tokio::time::timeout_at(deadline, h.events.recv()).await {
            match &ev {
                AgentEvent::TextDelta(_) => deltas += 1,
                AgentEvent::ReasoningDelta(_) => {}
                other => println!("{other:?}"),
            }
            if matches!(ev, AgentEvent::TurnComplete { .. } | AgentEvent::Exited) { break; }
        }
        println!("({deltas} text deltas)");
        let _ = h.commands.send(Command::Shutdown).await;
    });
}
