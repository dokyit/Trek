//! `trek-mcp` — a Model Context Protocol server over stdio.
//!
//! Usage: `trek-mcp computer` (macOS computer use), `trek-mcp simulator`
//! (iOS Simulator via `xcrun simctl` + AXe) or `trek-mcp orchestrate` (sub-agents,
//! run by the Trek that started the agent). JSON-RPC 2.0 messages are read
//! from stdin and written to stdout, one per line. Logs go to stderr only.

#[cfg(target_os = "macos")]
mod computer;
#[cfg(target_os = "macos")]
mod keys;
mod orchestrate;
mod rpc;
#[cfg(target_os = "macos")]
mod simulator;
#[cfg(target_os = "macos")]
mod util;

const USAGE: &str = "\
trek-mcp — Trek's MCP server (stdio, JSON-RPC 2.0, newline-delimited)

USAGE:
    trek-mcp computer     macOS computer use (screenshot, click, type, keys, windows)
    trek-mcp simulator    iOS Simulator control (simctl + AXe)
    trek-mcp orchestrate  sub-agents: delegate work to other agents and models in Trek
    trek-mcp --version
";

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    match arg.as_str() {
        #[cfg(target_os = "macos")]
        "computer" => serve(Box::new(computer::Computer::default())),
        #[cfg(not(target_os = "macos"))]
        "computer" | "simulator" | "sim" => {
            eprintln!("trek-mcp: the {arg} tools run only on macOS");
            std::process::exit(2);
        }
        #[cfg(target_os = "macos")]
        "simulator" | "sim" => serve(Box::new(simulator::Simulator::default())),
        "orchestrate" => {
            eprintln!("trek-mcp {} (orchestrate) ready on stdio", env!("CARGO_PKG_VERSION"));
            orchestrate::serve(orchestrate::Orchestrate::from_env());
        }
        "--version" | "-V" => println!("trek-mcp {}", env!("CARGO_PKG_VERSION")),
        "--help" | "-h" | "help" => print!("{USAGE}"),
        other => {
            eprintln!(
                "trek-mcp: unknown or missing tool family {other:?}\n\n{USAGE}"
            );
            std::process::exit(2);
        }
    }
}

/// Serve `tools` on stdin/stdout, one request at a time, until stdin closes. (`orchestrate`
/// serves itself, with calls side by side: the only family off macOS.)
#[cfg(target_os = "macos")]
fn serve(mut tools: Box<dyn rpc::ToolSet>) {
    use std::io::{BufRead, Write};

    eprintln!(
        "trek-mcp {} ({}) ready on stdio",
        env!("CARGO_PKG_VERSION"),
        tools.family()
    );

    let mut server = rpc::Server::new();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("trek-mcp: stdin read error: {e}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server.handle_line(&line, tools.as_mut()) {
            let mut out = stdout.lock();
            if writeln!(out, "{response}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    }
    eprintln!("trek-mcp: stdin closed, exiting");
}
