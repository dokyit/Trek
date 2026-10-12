//! One Trek per data folder: a second launch hands what it was started with (`trek://` links,
//! paths) to the Trek already running, over that Trek's pipe, and exits. This is the message
//! and the client's end; `server::serve_open` is the running Trek's.
//!
//! ```text
//! → {"open":{"version":1,"args":["trek://edit?…","C:\\src\\main.rs"],"background":false}}
//! ← {"ok":true}                      or {"error":"…"}
//! ```
//!
//! The answer comes once the running Trek's main thread has taken the arguments, so a Trek
//! that's hung doesn't answer, and the client gives up after the time it's given.

use crate::{Stream, encode, read_frame};
use serde_json::{Value, json};
use std::io::{BufReader, Write as _};
use std::path::Path;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

/// The most arguments one launch may hand over (each is a link or a path).
pub const MAX_ARGS: usize = 64;
/// The longest an answer may be: `{"ok":true}` or a short error.
const MAX_ANSWER: usize = 4096;

/// What a second launch hands over.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Open {
    /// `trek://` links as they came, and paths made absolute.
    pub args: Vec<String>,
    /// The launch was `TREK_BACKGROUND=1`: handle the arguments without bringing Trek forward.
    pub background: bool,
}

impl Open {
    pub fn to_frame(&self) -> Value {
        json!({ "open": { "version": crate::VERSION, "args": self.args, "background": self.background } })
    }

    /// An open request as a client sends it; `None` for anything else (another version, too
    /// many arguments, arguments that aren't strings).
    pub fn parse(v: &Value) -> Option<Open> {
        let o = v.get("open")?;
        if o.get("version")?.as_u64()? != crate::VERSION {
            return None;
        }
        let args = o.get("args")?.as_array()?;
        if args.len() > MAX_ARGS {
            return None;
        }
        let args = args.iter().map(|a| a.as_str().map(str::to_string)).collect::<Option<Vec<_>>>()?;
        Some(Open { args, background: o.get("background").and_then(Value::as_bool).unwrap_or(false) })
    }
}

/// Why a hand-over didn't happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardError {
    /// Nothing answering at that address (yet, or any more), or something there Trek won't talk
    /// to (a pipe another process serves): try again, or give up.
    Unreachable(String),
    /// The running Trek answered, and said no.
    Refused(String),
    /// Connected, but no answer within the time given: the running Trek is hung or very busy.
    TimedOut,
}

impl std::fmt::Display for ForwardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForwardError::Unreachable(e) => write!(f, "couldn't reach the running Trek: {e}"),
            ForwardError::Refused(e) => write!(f, "the running Trek said no: {e}"),
            ForwardError::TimedOut => write!(f, "the running Trek didn't answer in time"),
        }
    }
}

/// Hand `open` to the Trek listening at `address` and wait, at most `within`, for it to say it
/// has them. On Windows the pipe must be served by its own user and by the process its name
/// carries (`PipeStream::connect`), as for any other connection to Trek.
pub fn forward(address: &Path, open: &Open, within: Duration) -> Result<(), ForwardError> {
    let stream = Stream::connect(address).map_err(|e| ForwardError::Unreachable(e.to_string()))?;
    let reader = stream.try_clone().map_err(|e| ForwardError::Unreachable(e.to_string()))?;
    let handle = crate::handle_of(&stream).map_err(|e| ForwardError::Unreachable(e.to_string()))?;
    // Shut the connection down when the time is up: the read below then ends at once.
    let (finished, finish) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        let late = matches!(finish.recv_timeout(within), Err(RecvTimeoutError::Timeout));
        if late {
            let _ = handle.shutdown();
        }
        late
    });
    let answer = (|| {
        let mut w = stream;
        w.write_all(encode(&open.to_frame()).as_bytes()).map_err(|e| ForwardError::Unreachable(e.to_string()))?;
        read_frame(&mut BufReader::new(reader), MAX_ANSWER).map_err(|e| ForwardError::Unreachable(e.to_string()))
    })();
    drop(finished);
    let late = watchdog.join().unwrap_or(true);
    let line = match answer {
        _ if late => return Err(ForwardError::TimedOut),
        Ok(Some(line)) => line,
        Ok(None) => return Err(ForwardError::Unreachable("it closed the connection without answering".into())),
        Err(e) => return Err(e),
    };
    let v: Value = serde_json::from_str(&line).map_err(|_| ForwardError::Refused("it answered something that isn't JSON".into()))?;
    match (v.get("ok"), v.get("error")) {
        (Some(Value::Bool(true)), None) => Ok(()),
        (_, Some(e)) => Err(ForwardError::Refused(e.as_str().map(str::to_string).unwrap_or_else(|| e.to_string()))),
        _ => Err(ForwardError::Refused("it answered something unexpected".into())),
    }
}

/// The process serving the pipe named `address`, as its name says (`pipe_name`): the one a
/// second launch lets bring its window forward.
#[cfg(windows)]
pub fn served_by(address: &Path) -> Option<u32> {
    crate::windows::named_for(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_request_round_trips_and_junk_is_refused() {
        let open = Open { args: vec!["trek://edit?path=%2Fx".into(), "/tmp/a b.rs".into()], background: true };
        assert_eq!(Open::parse(&open.to_frame()), Some(open));
        assert_eq!(Open::parse(&json!({"open": {"version": crate::VERSION, "args": []}})), Some(Open::default()), "no arguments: just come forward");
        for junk in [
            json!({"open": {"version": 99, "args": []}}),
            json!({"open": {"version": crate::VERSION, "args": [1, 2]}}),
            json!({"open": {"version": crate::VERSION, "args": "trek://edit"}}),
            json!({"open": {"version": crate::VERSION, "args": vec!["x"; MAX_ARGS + 1]}}),
            json!({"hello": {"version": 1, "token": "t", "session": "s"}}),
            json!(null),
        ] {
            assert_eq!(Open::parse(&junk), None, "{junk}");
        }
    }

    #[test]
    fn nothing_listening_is_unreachable_at_once() {
        #[cfg(unix)]
        let address = std::env::temp_dir().join(format!("trek-ipc-none-{}.sock", std::process::id()));
        #[cfg(windows)]
        let address = crate::pipe_name(7).unwrap();
        let started = std::time::Instant::now();
        let err = forward(&address, &Open::default(), Duration::from_secs(5)).unwrap_err();
        assert!(matches!(err, ForwardError::Unreachable(_)), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
