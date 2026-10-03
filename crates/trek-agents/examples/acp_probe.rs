//! Probe installed ACP agents: `cargo run -p trek-agents --example acp_probe -- [id...]`
//! Answers come from what each agent last reported (under Trek's data folder; set
//! `TREK_DATA_DIR` to probe afresh without touching the app's).
use trek_agents::acp_probe;

fn main() {
    let mut ids: Vec<String> = std::env::args().skip(1).collect();
    if ids.is_empty() {
        ids = ["opencode", "kimi", "github-copilot", "grok", "devin"].map(String::from).to_vec();
    }
    trek_core::runtime().block_on(async {
        for id in ids {
            let started = std::time::Instant::now();
            match acp_probe(&id).await {
                Ok(info) => {
                    let names: Vec<&str> = info.models.iter().take(6).map(|m| m.id.as_str()).collect();
                    println!(
                        "{id}: ok in {:.1}s, needs_auth={}, {} models {:?}{}, auth={:?}",
                        started.elapsed().as_secs_f32(),
                        info.needs_auth,
                        info.models.len(),
                        names,
                        if info.models.len() > 6 { " …" } else { "" },
                        info.auth_methods,
                    );
                }
                Err(e) => println!("{id}: error after {:.1}s: {e:#}", started.elapsed().as_secs_f32()),
            }
        }
    });
}
