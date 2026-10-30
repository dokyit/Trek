//! How a harness wires the `script` tool: it already has something that runs commands and
//! touches files for the model's one-at-a-time tool calls; an adapter turns that into a
//! `ScriptHost`, and a `script` tool call becomes `run_script` with the model's code.
//!
//! `cargo run -p trek-script --example harness` runs a program against a scratch directory.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use trek_script::{ScriptHost, ScriptLimits, async_trait, run_script};

/// Stands in for the harness's own executor (in Trek, `direct.rs`'s `ToolExecutor`).
struct Executor {
    cwd: PathBuf,
}

impl Executor {
    async fn exec(&self, command: &str, timeout_s: u64) -> Result<(String, String, i32), String> {
        let mut cmd = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", command]);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", command]);
            c
        };
        // A script's call that times out or is aborted is dropped: take the process with it.
        cmd.current_dir(&self.cwd).kill_on_drop(true);
        let out = tokio::time::timeout(std::time::Duration::from_secs(timeout_s), cmd.output())
            .await
            .map_err(|_| format!("timed out after {timeout_s} s"))?
            .map_err(|e| e.to_string())?;
        Ok((
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        ))
    }

    fn path(&self, p: &str) -> PathBuf {
        self.cwd.join(p)
    }

    async fn read(&self, path: &Path) -> Result<String, String> {
        tokio::fs::read_to_string(path)
            .await
            .map_err(|e| e.to_string())
    }

    async fn write(&self, path: &Path, content: &str) -> Result<(), String> {
        tokio::fs::write(path, content)
            .await
            .map_err(|e| e.to_string())
    }
}

/// The adapter: the script's tool names and argument shapes, onto the executor.
struct Host(Executor);

#[async_trait]
impl ScriptHost for Host {
    async fn call(&self, tool: &str, args: Value) -> Result<Value, String> {
        let str_arg = |k: &str| {
            args[k]
                .as_str()
                .map(str::to_owned)
                .ok_or(format!("missing {k}"))
        };
        match tool {
            "bash" => {
                let timeout = args["timeout_s"].as_u64().unwrap_or(120);
                let (stdout, stderr, exit_code) =
                    self.0.exec(&str_arg("command")?, timeout).await?;
                Ok(json!({ "stdout": stdout, "stderr": stderr, "exit_code": exit_code }))
            }
            "read" => {
                let text = self.0.read(&self.0.path(&str_arg("path")?)).await?;
                let start = args["start_line"].as_u64().unwrap_or(1).max(1) as usize;
                let end = args["end_line"].as_u64().map(|e| e as usize);
                let lines = text
                    .lines()
                    .skip(start - 1)
                    .take(end.map_or(usize::MAX, |e| e + 1 - start));
                Ok(json!({ "content": lines.collect::<Vec<_>>().join("\n") }))
            }
            "write" => {
                self.0
                    .write(&self.0.path(&str_arg("path")?), &str_arg("content")?)
                    .await?;
                Ok(json!({}))
            }
            "edit" => {
                let path = self.0.path(&str_arg("path")?);
                let (old, new) = (str_arg("old")?, str_arg("new")?);
                let text = self.0.read(&path).await?;
                match text.matches(old.as_str()).count() {
                    1 => self.0.write(&path, &text.replacen(&old, &new, 1)).await?,
                    0 => return Err("old text not found".into()),
                    n => return Err(format!("old text found {n} times; give more context")),
                }
                Ok(json!({}))
            }
            // Anything else the harness offers (MCP tools, search, …) would be matched here.
            _ => Err(format!("no tool named {tool}")),
        }
    }
}

const PROGRAM: &str = r#"
// Write ten files at once, fix one, then report only what matters.
await Promise.all(Array.from({length: 10}, (_, i) =>
  write(`note-${i}.txt`, `note ${i}\nstatus: ${i % 3 ? "ok" : "todo"}\n`)));
await edit("note-0.txt", "status: todo", "status: done");
const {stdout} = await bash("grep -l 'status: todo' note-*.txt | sort");
const todo = stdout.trim().split("\n");
print(`${todo.length} files still todo: ${todo.join(", ")}`);
print((await read("note-0.txt", {start: 2})).trim());
"#;

#[tokio::main]
async fn main() {
    let dir = std::env::temp_dir().join(format!("trek-script-example-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let host = Arc::new(Host(Executor { cwd: dir.clone() }));
    let limits = ScriptLimits {
        log_dir: Some(dir.join("logs")),
        ..ScriptLimits::default()
    };

    let result = run_script(PROGRAM, host, limits).await;
    // What goes back to the model as the tool result:
    print!("{}", result.to_model_text());
    // And what the harness keeps for the UI:
    eprintln!(
        "{} calls {:?} in {} ms, log at {}",
        result.nested_calls,
        result.calls_by_tool,
        result.elapsed_ms,
        result
            .log_path
            .as_deref()
            .map(Path::display)
            .map(|d| d.to_string())
            .unwrap_or_default()
    );
    let _ = std::fs::remove_dir_all(dir);
}
