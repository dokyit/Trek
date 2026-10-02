//! `trek-mcp` — a Model Context Protocol server over stdio.
//!
//! Usage: `trek-mcp computer` (macOS computer use) or `trek-mcp simulator`
//! (iOS Simulator via `xcrun simctl` + AXe). JSON-RPC 2.0 messages are read
//! from stdin and written to stdout, one per line. Logs go to stderr only.

mod computer;
mod keys;
mod rpc;
mod simulator;
mod util;

use std::io::{BufRead, Write};

const USAGE: &str = "\
trek-mcp — Trek's MCP server (stdio, JSON-RPC 2.0, newline-delimited)

USAGE:
    trek-mcp computer     macOS computer use (screenshot, click, type, keys, windows)
    trek-mcp simulator    iOS Simulator control (simctl + AXe)
    trek-mcp --version
";

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    let mut tools: Box<dyn rpc::ToolSet> = match arg.as_str() {
        "computer" => Box::new(computer::Computer::default()),
        "simulator" | "sim" => Box::new(simulator::Simulator::default()),
        "--version" | "-V" => {
            println!("trek-mcp {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        "--help" | "-h" | "help" => {
            print!("{USAGE}");
            return;
        }
        other => {
            eprintln!(
                "trek-mcp: unknown or missing tool family {other:?}\n\n{USAGE}"
            );
            std::process::exit(2);
        }
    };

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
