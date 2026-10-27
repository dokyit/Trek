//! `trek-mcp` — a Model Context Protocol server over stdio.
//!
//! Usage: `trek-mcp computer` (computer use on macOS and Windows), `trek-mcp simulator`
//! (iOS Simulator via `xcrun simctl` + AXe), `trek-mcp video` (`watch_video`: frames and a
//! transcript of a video, through ffmpeg), `trek-mcp orchestrate` (sub-agents, run by the Trek
//! that started the agent), `trek-mcp browser` (browser use in that Trek's own Browser tool) or
//! `trek-mcp phone` (the user's real iPhone, through that Trek's iPhone tool).
//! JSON-RPC 2.0 messages are read from stdin and written to stdout, one per line. Logs go to
//! stderr only.

// Trek's Browser tool has a web view on macOS and Windows only.
#[cfg(any(target_os = "macos", windows))]
mod browser;
#[cfg(any(target_os = "macos", windows))]
mod computer;
#[cfg(any(target_os = "macos", windows))]
mod desktop;
#[cfg(any(target_os = "macos", windows))]
mod keys;
mod orchestrate;
#[cfg(target_os = "macos")]
mod phone;
mod rpc;
#[cfg(target_os = "macos")]
mod simulator;
#[cfg(any(target_os = "macos", windows))]
mod util;
#[cfg(any(target_os = "macos", windows))]
mod video;

const USAGE: &str = "\
trek-mcp — Trek's MCP server (stdio, JSON-RPC 2.0, newline-delimited)

USAGE:
    trek-mcp computer     computer use (screenshot, click, type, keys, windows)
    trek-mcp simulator    iOS Simulator control (simctl + AXe), macOS only
    trek-mcp video        watch_video: a video's frames and transcript (needs ffmpeg)
    trek-mcp orchestrate  sub-agents: delegate work to other agents and models in Trek
    trek-mcp browser      browser use in Trek's Browser tool (run by the Trek that started the agent)
    trek-mcp phone        the user's real iPhone through Trek's iPhone tool (macOS)
    trek-mcp phone-device the iPhone's apps and files over USB, no Xcode (run alone for help)
    trek-mcp --version
";

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    match arg.as_str() {
        #[cfg(any(target_os = "macos", windows))]
        "computer" => serve(Box::new(computer::Computer::default())),
        #[cfg(not(any(target_os = "macos", windows)))]
        "computer" => {
            eprintln!("trek-mcp: the computer tools run only on macOS and Windows");
            std::process::exit(2);
        }
        #[cfg(not(target_os = "macos"))]
        "simulator" | "sim" => {
            eprintln!("trek-mcp: the {arg} tools run only on macOS");
            std::process::exit(2);
        }
        #[cfg(target_os = "macos")]
        "simulator" | "sim" => serve(Box::new(simulator::Simulator::default())),
        #[cfg(any(target_os = "macos", windows))]
        "video" => serve(Box::new(video::Video::default())),
        #[cfg(not(any(target_os = "macos", windows)))]
        "video" => {
            eprintln!("trek-mcp: the video tools run only on macOS and Windows");
            std::process::exit(2);
        }
        // `watch_video`'s Speech pass, in a process of its own (see `video::speech_child`).
        #[cfg(target_os = "macos")]
        "transcribe" => match std::env::args().nth(2) {
            Some(file) => video::transcribe_cli(&file),
            None => {
                eprintln!("trek-mcp transcribe <audio file>");
                std::process::exit(2);
            }
        },
        #[cfg(any(target_os = "macos", windows))]
        "browser" => serve(Box::new(browser::Browser::from_env())),
        #[cfg(not(any(target_os = "macos", windows)))]
        "browser" => {
            eprintln!("trek-mcp: the browser tools run only on macOS and Windows");
            std::process::exit(2);
        }
        #[cfg(target_os = "macos")]
        "phone" => serve(Box::new(phone::Phone::from_env())),
        #[cfg(target_os = "macos")]
        "phone-device" => {
            let rest: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(phone::device_cli(&rest));
        }
        #[cfg(not(target_os = "macos"))]
        "phone" | "phone-device" => {
            eprintln!("trek-mcp: the {arg} tools run only on macOS");
            std::process::exit(2);
        }
        "orchestrate" => {
            eprintln!(
                "trek-mcp {} (orchestrate) ready on stdio",
                env!("CARGO_PKG_VERSION")
            );
            orchestrate::serve(orchestrate::Orchestrate::from_env());
        }
        "--version" | "-V" => println!("trek-mcp {}", env!("CARGO_PKG_VERSION")),
        "--help" | "-h" | "help" => print!("{USAGE}"),
        other => {
            eprintln!("trek-mcp: unknown or missing tool family {other:?}\n\n{USAGE}");
            std::process::exit(2);
        }
    }
}

/// Serve `tools` on stdin/stdout, one request at a time, until stdin closes. (`orchestrate`
/// serves itself, with calls side by side.)
#[cfg(any(target_os = "macos", windows))]
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
            if writeln!(out, "{response}")
                .and_then(|_| out.flush())
                .is_err()
            {
                break;
            }
        }
    }
    eprintln!("trek-mcp: stdin closed, exiting");
}
