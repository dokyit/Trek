//! What ending an agent costs a caller that took its stdin, as every session does. On Windows the
//! end of stdin is the only gentle stop (there's no TERM), so a caller that keeps stdin across
//! `terminate` waits out the whole grace before the agent is killed. The stand-in exits when its
//! stdin ends and, on Unix, ignores TERM, so it stops the way an agent does on Windows on both.

use super::*;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt as _, BufReader};

/// A stand-in agent, running, with its stdin taken as the sessions take it.
async fn reader() -> (GroupChild, tokio::process::ChildStdin) {
    let mut command = if cfg!(windows) {
        // `more` reads stdin to its end.
        let mut c = tokio::process::Command::new("cmd.exe");
        c.args(["/d", "/c", "echo ready& more >nul"]);
        c
    } else {
        // The TERM it ignores stays ignored in `cat`.
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", "trap '' TERM; echo ready; cat >/dev/null"]);
        c
    };
    command.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = spawn_group(&mut command).unwrap();
    let stdin = child.stdin.take().unwrap();
    let mut line = String::new();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    tokio::time::timeout(Duration::from_secs(30), stdout.read_line(&mut line)).await.expect("the stand-in started").unwrap();
    assert_eq!(line.trim(), "ready");
    (child, stdin)
}

#[tokio::test]
async fn with_stdin_closed_first_an_agent_stops_at_once() {
    let (mut child, stdin) = reader().await;
    let group = child.group;
    drop(stdin);
    let started = Instant::now();
    child.terminate().await;
    let took = started.elapsed();
    eprintln!("terminate with stdin closed first: {took:?}");
    assert!(took < Duration::from_secs(1), "the end of stdin stopped it, not the grace's kill: {took:?}");
    assert!(!trek_core::procs::live().contains(&group));
}

#[tokio::test]
async fn with_stdin_held_an_agent_is_waited_out_then_killed() {
    let (mut child, stdin) = reader().await;
    let group = child.group;
    let started = Instant::now();
    child.terminate().await;
    let took = started.elapsed();
    eprintln!("terminate with stdin held: {took:?}");
    assert!(took >= Duration::from_millis(1900), "nothing but the grace's kill could stop it: {took:?}");
    assert!(took < Duration::from_secs(10), "{took:?}");
    assert!(!trek_core::procs::live().contains(&group));
    drop(stdin);
}

#[tokio::test]
async fn kill_now_skips_the_grace() {
    let (mut child, stdin) = reader().await;
    let group = child.group;
    let started = Instant::now();
    child.kill_now().await;
    let took = started.elapsed();
    eprintln!("kill_now with stdin held: {took:?}");
    assert!(took < Duration::from_secs(1), "{took:?}");
    assert!(!trek_core::procs::live().contains(&group));
    drop(stdin);
}
