//! End-to-end: `cargo run -p trek-agents --example ping -- claude|codex`
use trek_agents::{AgentEvent, Command, SessionConfig, start};
use trek_core::{AgentId, Effort, HandHolding};

fn main() {
    let which = std::env::args().nth(1).unwrap_or("claude".into());
    let (agent, model) = match which.as_str() {
        "codex" => (AgentId::Codex, "gpt-5.6-luna"),
        _ => (AgentId::ClaudeCode, "claude-haiku-4-5"),
    };
    let h = start(SessionConfig {
        agent, cwd: "/tmp/trek-e2e".into(), model: Some(model.into()), effort: Effort::Low,
        hand_holding: HandHolding::Supervised, plan: false, resume: None,
    });
    trek_core::runtime().block_on(async {
        h.commands.send(Command::Prompt("Reply with just the word: pong".into())).await.unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
        while let Ok(Ok(ev)) = tokio::time::timeout_at(deadline, h.events.recv()).await {
            match &ev {
                AgentEvent::TextDelta(_) | AgentEvent::ReasoningDelta(_) => {}
                other => println!("{other:?}"),
            }
            if matches!(ev, AgentEvent::TurnComplete { .. } | AgentEvent::Exited) { break; }
        }
        let _ = h.commands.send(Command::Shutdown).await;
    });
}
