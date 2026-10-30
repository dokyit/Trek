//! The wiring point for a fifth Direct tool, `script`: a program the model writes that calls
//! the other tools itself (the `trek-script` crate provides it). A session offers it only when
//! `DirectOptions::script` holds a host; `tool_defs` then lists its definition after the four
//! built in, and `execute_tool` hands every `script` call to `ScriptHost::run`.
//!
//! A host is built on the session's own [`ToolExecutor`], so a script's tool calls run where the
//! model's do (this machine, or the benchmark's sandbox). They don't go through Trek's approvals
//! (`HandHolding`): only give a session a host when it runs with Full access.

use super::executor::ToolExecutor;
use anyhow::Result;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// What a `script` call came to.
#[derive(Debug, Clone, Default)]
pub struct ScriptOutcome {
    /// What the model is told (capped by the host, like `bash` output is).
    pub output: String,
    pub ok: bool,
    /// Calls the script made back into tools or models, for the benchmark's counts.
    pub nested_calls: u32,
}

pub trait ScriptHost: Send + Sync {
    /// The tool's definition, in the OpenAI function shape `direct_tools` uses (it's turned
    /// into the Messages API's shape for Anthropic). Its name must be `script`.
    fn tool_def(&self) -> Value;
    /// A paragraph for the system prompt about when to use it, if it needs one.
    fn prompt_note(&self) -> Option<String> {
        None
    }
    /// Run one call: `args` as the model sent them, `cwd` the session's folder. An `Err` is
    /// told to the model as the tool's failure, unless it's an `ExecutorGone` (the turn ends).
    fn run<'a>(&'a self, args: &'a Value, cwd: &'a Path) -> BoxFuture<'a, Result<ScriptOutcome>>;
}

/// The stand-in until `trek-script` is built: it offers the tool and says so when called.
pub struct UnbuiltScript {
    #[allow(dead_code)]
    executor: Arc<dyn ToolExecutor>,
}

impl UnbuiltScript {
    pub fn new(executor: Arc<dyn ToolExecutor>) -> Self {
        UnbuiltScript { executor }
    }
}

impl ScriptHost for UnbuiltScript {
    fn tool_def(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "script",
                "description": "Run a short program that calls the other tools itself. (Not available in this build.)",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "code": { "type": "string", "description": "The program to run." }
                    },
                    "required": ["code"]
                }
            }
        })
    }

    fn run<'a>(&'a self, _args: &'a Value, _cwd: &'a Path) -> BoxFuture<'a, Result<ScriptOutcome>> {
        Box::pin(async {
            Ok(ScriptOutcome {
                output: "script tool not built".into(),
                ok: false,
                nested_calls: 0,
            })
        })
    }
}
