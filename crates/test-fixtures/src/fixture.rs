//! `fixture <verb> …` — the small utilities tests used to get from shell tools:
//!   sleep <secs>           wait, then exit 0
//!   cat [--delay <secs>] [--stderr <text>]
//!                          wait, then stdin to stdout, then `text` on stderr
//!   echo <text…>           args joined by spaces on stdout, with a newline
//!   print <text…>          the same without the newline
//!   stderr <text…>         the same on stderr
//!   exit [<code>]          exit with `code` (0 when absent or not a number); later args ignored
//!   fake-acp               run the stand-in ACP agent
//!   fake-mcp [<mode>]      run the stand-in MCP server
//!   serve <dir> [<port>]   the files in `dir` over HTTP on 127.0.0.1 (see `serve`)
//!   minisign-keygen <dir>  a test update-signing key (see `minisign`); prints the key line
//!   minisign-sign <seed file> <file> <trusted comment>
//!                          sign `file` into `<file>.minisig` with that key

use std::io::{Read, Write};

pub fn run() -> i32 {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("sleep") => {
            let secs: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            std::thread::sleep(std::time::Duration::from_secs_f64(secs));
            0
        }
        Some("cat") => {
            let rest: Vec<String> = args.collect();
            let flag = |name: &str| rest.iter().position(|a| a == name).and_then(|i| rest.get(i + 1));
            if let Some(secs) = flag("--delay") {
                std::thread::sleep(std::time::Duration::from_secs_f64(secs.parse().unwrap_or(0.0)));
            }
            let mut buf = Vec::new();
            std::io::stdin().lock().read_to_end(&mut buf).unwrap();
            let mut out = std::io::stdout().lock();
            out.write_all(&buf).unwrap();
            out.flush().unwrap();
            if let Some(text) = flag("--stderr") {
                eprintln!("{text}");
            }
            0
        }
        Some("echo") => {
            println!("{}", args.collect::<Vec<_>>().join(" "));
            0
        }
        Some("print") => {
            print!("{}", args.collect::<Vec<_>>().join(" "));
            std::io::stdout().flush().unwrap();
            0
        }
        Some("stderr") => {
            eprintln!("{}", args.collect::<Vec<_>>().join(" "));
            0
        }
        Some("exit") => args.next().and_then(|c| c.parse().ok()).unwrap_or(0),
        Some("fake-acp") => {
            crate::fake_acp::run();
            0
        }
        Some("fake-mcp") => {
            crate::fake_mcp::run(args.next());
            0
        }
        Some("serve") => match args.next() {
            Some(dir) => crate::serve::run(dir.into(), args.next().and_then(|p| p.parse().ok()).unwrap_or(0)),
            None => {
                eprintln!("fixture serve <dir> [<port>]");
                2
            }
        },
        Some("minisign-keygen") => match args.next().map(|d| crate::minisign::keygen(std::path::Path::new(&d))) {
            Some(Ok(line)) => {
                println!("{line}");
                0
            }
            Some(Err(e)) => {
                eprintln!("fixture minisign-keygen: {e}");
                1
            }
            None => {
                eprintln!("fixture minisign-keygen <dir>");
                2
            }
        },
        Some("minisign-sign") => {
            let rest: Vec<String> = args.collect();
            let [seed, file, comment] = rest.as_slice() else {
                eprintln!("fixture minisign-sign <seed file> <file> <trusted comment>");
                return 2;
            };
            match crate::minisign::sign(seed.as_ref(), file.as_ref(), comment) {
                Ok(_) => 0,
                Err(e) => {
                    eprintln!("fixture minisign-sign: {e}");
                    1
                }
            }
        }
        other => {
            eprintln!("fixture: expected sleep, cat, echo, print, stderr, exit, fake-acp, fake-mcp, serve, minisign-keygen or minisign-sign, not {other:?}");
            2
        }
    }
}
