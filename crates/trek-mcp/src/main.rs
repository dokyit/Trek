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
    trek-mcp mac-update [--status] [--app PATH]
                          update the installed Trek while it's closed (macOS)
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
        #[cfg(target_os = "macos")]
        "mac-update" => {
            let rest: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(mac_update(&rest));
        }
        #[cfg(not(target_os = "macos"))]
        "phone" | "phone-device" | "mac-update" => {
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

/// `trek-mcp mac-update`: Trek's own updater for the installed app while Trek is closed
/// (`trek_core::update::closed`), for the phone updater's LaunchAgent. `--app` is the bundle (by
/// default the one this `trek-mcp` is in, else /Applications/Trek.app); `--status` says what it
/// would do. Prints one line; exits 1 when the update failed, 2 on bad arguments.
#[cfg(target_os = "macos")]
fn mac_update(args: &[String]) -> i32 {
    let mut status = false;
    let mut app = None;
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--status" => status = true,
            "--app" => app = args.next().map(std::path::PathBuf::from),
            other => {
                eprintln!("trek-mcp mac-update: unknown argument {other:?}\n\n{USAGE}");
                return 2;
            }
        }
    }
    let app = app
        .or_else(trek_core::update::running_bundle)
        .unwrap_or_else(|| "/Applications/Trek.app".into());
    let run = trek_core::update::closed::update(&app, !status);
    match trek_core::runtime().block_on(run) {
        Ok(line) => {
            println!("{line}");
            0
        }
        Err(e) => {
            println!("{e:#}");
            1
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

/// What each tool family adds to every request of an agent given it: its server instructions
/// (Claude Code puts them in its system prompt) and its tool schemas (sent in full by agents that
/// don't defer MCP tools, such as OpenCode). docs/PROMPT_BUDGET.md has the measured tokens.
///
/// To raise a budget deliberately: re-measure (the doc says how), update its table, and set the
/// constant to the new size plus about 10% headroom. Every character goes out with every request
/// of every thread, so first look for words to cut: say a thing once, in the tool's description or
/// in the instructions, not both.
#[cfg(all(test, target_os = "macos"))]
mod prompt_budget {
    use crate::rpc::ToolSet;

    /// (family, instructions' chars, tool schemas' chars as MCP JSON) at most.
    const BUDGETS: &[(&str, usize, usize)] = &[
        ("computer", 650, 4_600),
        ("computer-background", 1_000, 5_400),
        ("simulator", 680, 5_100),
        ("video", 240, 1_250),
        ("orchestrate", 1_050, 10_000),
        ("browser", 950, 7_300),
        ("phone", 1_230, 6_650),
    ];
    /// Everything on (computer use in the background and every default family) together.
    const ALL_CHARS: usize = 40_500;

    fn families() -> Vec<(&'static str, Box<dyn ToolSet>)> {
        vec![
            ("computer", Box::new(crate::computer::Computer::default())),
            (
                "computer-background",
                Box::new(crate::computer::Computer::in_background()),
            ),
            (
                "simulator",
                Box::new(crate::simulator::Simulator::default()),
            ),
            ("video", Box::new(crate::video::Video::default())),
            (
                "orchestrate",
                Box::new(crate::orchestrate::Orchestrate::from_env()),
            ),
            ("browser", Box::new(crate::browser::Browser::from_env())),
            ("phone", Box::new(crate::phone::Phone::from_env())),
        ]
    }

    #[test]
    fn prompt_budget_tool_families_stay_within_budget() {
        let mut all = 0;
        for (name, set) in families() {
            let (_, max_instructions, max_tools) =
                BUDGETS.iter().find(|(n, ..)| *n == name).copied().unwrap();
            let instructions = set.instructions().chars().count();
            let tools: usize = set
                .tools()
                .iter()
                .map(|t| t.to_json().to_string().chars().count())
                .sum();
            println!("{name}: instructions {instructions}, tools {tools}");
            assert!(
                instructions <= max_instructions,
                "trek-mcp {name}'s instructions grew to {instructions} chars (budget \
                 {max_instructions}); see prompt_budget in main.rs before raising it"
            );
            assert!(
                tools <= max_tools,
                "trek-mcp {name}'s tool schemas grew to {tools} chars (budget {max_tools}); see \
                 prompt_budget in main.rs before raising it"
            );
            if name != "computer" {
                all += instructions + tools;
            }
        }
        assert!(
            all <= ALL_CHARS,
            "trek-mcp's families grew to {all} chars together (budget {ALL_CHARS})"
        );
    }
}
