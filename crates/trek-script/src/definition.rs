//! The `script` tool as the model sees it. Every word here is paid for on every request, so the
//! description says when to reach for it, what the program can call, and the limits, and stops.

use serde_json::{Value, json};

const DESCRIPTION: &str = "\
Run a JavaScript program that calls your tools for you. Use it instead of separate tool calls \
when a task takes more than two or three: loops over files, batches of commands, filtering or \
counting large output, checking your own work. Tool results stay in the program's variables; \
only what it prints comes back to you, so print just what you need.

Available (await all but print): bash(cmd, {timeout: seconds}) -> {stdout, stderr, exit_code}; \
read(path, {start, end}) -> text; write(path, text); edit(path, old, new); tool(name, args) for \
any other tool; sleep(ms); print(...) or console.log(...). Top-level await works; run \
independent calls together with Promise.all (8 at a time). A failed call throws. A returned \
value is printed. The program ends when its body does: calls it didn't await get 2 more \
seconds, then are stopped.

No import or require, and no other file, network or system access. Limits: 5 minutes (then \
you get the output so far), 16 KB of printed output (head and tail kept), 1,000 tool calls; a \
single result over 2 MB loses its middle.";

fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "code": {
                "type": "string",
                "description": "The JavaScript program."
            },
            "description": {
                "type": "string",
                "description": "A few words on what it does, shown to the user."
            }
        },
        "required": ["code"]
    })
}

/// The OpenAI function-call form: `{name, description, parameters}`. Chat Completions wants it
/// inside `{"type": "function", "function": …}`.
pub fn tool_definition() -> Value {
    json!({
        "name": "script",
        "description": DESCRIPTION,
        "parameters": parameters(),
    })
}

/// The Anthropic Messages form: `{name, description, input_schema}`.
pub fn anthropic_tool_definition() -> Value {
    json!({
        "name": "script",
        "description": DESCRIPTION,
        "input_schema": parameters(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_stays_small() {
        // About four characters a token; the brief is under 300 tokens of text.
        let text = tool_definition().to_string();
        assert!(DESCRIPTION.len() / 4 < 300, "{} chars", DESCRIPTION.len());
        assert!(text.len() / 4 < 360, "{} chars", text.len());
        assert_eq!(
            anthropic_tool_definition()["input_schema"]["required"][0],
            "code"
        );
    }
}
