//! End-to-end: `cargo run -p trek-agents --example ping -- claude|codex|opencode [image.png]`
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
        hand_holding: HandHolding::Supervised, plan: false, read_only: false, resume: None, resume_at: None, fork: false, recap: None, fast: None, mcp_servers: vec![], instructions: None,
    });
    trek_core::runtime().block_on(async {
        let images: Vec<std::path::PathBuf> = std::env::args().skip(2).map(Into::into).collect();
        let text = if images.is_empty() { "Reply with just the word: pong" } else { "Reply with just the main color of the image." };
        h.commands.send(Command::Prompt { text: text.into(), images }).await.unwrap();
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
