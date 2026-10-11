//! `fixture serve <dir> [port]`: the files directly in `dir` over plain HTTP on 127.0.0.1 (never
//! another interface, so no firewall prompt), for updater end-to-end runs. Prints
//! `serving on http://127.0.0.1:<port>` first, then `GET /<name> <status>` per request.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

pub fn run(dir: PathBuf, port: u16) -> i32 {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("fixture serve: {e}");
            return 1;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    println!("serving on http://127.0.0.1:{port}");
    let _ = std::io::stdout().flush();
    serve(listener, dir);
    0
}

/// Answer requests on `listener` from `dir`, each on a thread of its own, for as long as it's open.
pub fn serve(listener: TcpListener, dir: PathBuf) {
    for stream in listener.incoming().flatten() {
        let dir = dir.clone();
        std::thread::spawn(move || {
            if let Some(line) = answer(stream, &dir) {
                println!("{line}");
                let _ = std::io::stdout().flush();
            }
        });
    }
}

/// Answer one request; the log line.
fn answer(mut stream: TcpStream, dir: &Path) -> Option<String> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut request = String::new();
    reader.read_line(&mut request).ok()?;
    // The headers say nothing this needs.
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 || header.trim().is_empty() {
            break;
        }
    }
    let mut parts = request.split_whitespace();
    let (method, target) = (parts.next()?, parts.next()?);
    let name = target.trim_start_matches('/');
    // A file directly in `dir`, and only that.
    let plain = !name.is_empty() && !name.contains(['/', '\\', ':']) && name != "..";
    let file = dir.join(name);
    let (status, body) = match std::fs::read(&file) {
        Ok(body) if plain && method == "GET" && file.is_file() => ("200 OK", body),
        _ => ("404 Not Found", Vec::new()),
    };
    let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n", body.len());
    stream.write_all(head.as_bytes()).ok()?;
    stream.write_all(&body).ok()?;
    stream.flush().ok()?;
    Some(format!("{method} {target} {}", &status[..3]))
}
