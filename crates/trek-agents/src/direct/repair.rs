//! Making sense of the tool calls models actually send. Strong models send clean JSON under the
//! right names; the rest send `{…}{}`, trailing commas, raw newlines in strings, `file_path`
//! for `path`, `Bash` for `bash`, or the call as text in one of half a dozen house formats. A
//! call that can be understood runs; one that can't gets an error the model can fix from, and
//! a call cut off by the output limit never runs at all.

use super::AccumulatedToolCall;
use serde_json::{Map, Value, json};
use std::collections::HashSet;

/// What a call's raw argument text turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub enum Args {
    /// Valid as sent.
    Clean(Value),
    /// Valid after a repair: this is what's run, and what history says was sent.
    Repaired(Value),
    /// Cut off part way: running it would run half a command or half a file.
    Truncated,
    /// Not JSON, in a way a repair won't guess at.
    Invalid(String),
}

impl Args {
    pub fn value(&self) -> Option<&Value> {
        match self {
            Args::Clean(v) | Args::Repaired(v) => Some(v),
            _ => None,
        }
    }
}

/// Parse a call's arguments, repairing what can be repaired without guessing at the content. Each
/// step is tried in turn and the first that parses wins, so a clean call costs one parse.
pub fn parse_args(raw: &str) -> Args {
    let text = raw.trim();
    // Providers send nothing for a call without parameters.
    if text.is_empty() {
        return Args::Clean(json!({}));
    }
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return match v {
            Value::Object(_) => Args::Clean(v),
            // Encoded twice over: a string holding the object.
            Value::String(inner) => match parse_args(&inner) {
                Args::Clean(v) => Args::Repaired(v),
                other => other,
            },
            Value::Null => Args::Repaired(json!({})),
            other => Args::Invalid(format!("expected a JSON object, got {}", kind(&other))),
        };
    }
    // `{…}{}` and `{…}garbage`: a real model's way of ending a call. The first value is the call.
    if let Some(first) = first_object(text) {
        return Args::Repaired(first);
    }
    let attempts = [
        escape_string_controls(text),
        strip_trailing_commas(&escape_string_controls(text)),
        fix_invalid_escapes(&strip_trailing_commas(&escape_string_controls(text))),
    ];
    for attempt in &attempts {
        if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(attempt) {
            return Args::Repaired(v);
        }
    }
    // Text that stops inside a string, or doesn't end on a closer, was cut off. Closing it would
    // run a command or write a file the model never finished.
    let end = text.chars().next_back();
    if text.starts_with(['{', '[']) && !matches!(end, Some('}' | ']')) {
        return Args::Truncated;
    }
    let cleaned = fix_invalid_escapes(&strip_trailing_commas(&escape_string_controls(text)));
    match scan(&cleaned) {
        Scan::InString => return Args::Truncated,
        Scan::Open(closers) if !closers.is_empty() => {
            let mut closed = cleaned.clone();
            closed.extend(closers.iter().rev());
            if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(&closed) {
                return Args::Repaired(v);
            }
        }
        _ => {}
    }
    // Too many closers: `{"a":1}}`.
    let mut shorter = cleaned.as_str();
    for _ in 0..50 {
        let Some(cut) = shorter.strip_suffix(['}', ']']) else {
            break;
        };
        shorter = cut.trim_end();
        if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(shorter) {
            return Args::Repaired(v);
        }
    }
    let why = serde_json::from_str::<Value>(text)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_else(|| "not a JSON object".to_string());
    Args::Invalid(why)
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The first JSON object in `text` that has anything in it, when `text` is several values one
/// after the other: `{…}{}` and `{}{…}` are both seen in the wild.
fn first_object(text: &str) -> Option<Value> {
    let mut stream = serde_json::Deserializer::from_str(text).into_iter::<Value>();
    let Some(Ok(first @ Value::Object(_))) = stream.next() else {
        return None;
    };
    if first.as_object().is_some_and(|o| !o.is_empty()) {
        return Some(first);
    }
    for next in stream {
        match next {
            Ok(v @ Value::Object(_)) if v.as_object().is_some_and(|o| !o.is_empty()) => {
                return Some(v);
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    Some(first)
}

/// Raw newlines, tabs and other control characters inside strings, which strict JSON refuses and
/// many models send when writing a file.
fn escape_string_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut in_string, mut escaped) = (false, false);
    for c in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            } else if (c as u32) < 0x20 {
                match c {
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    _ => out.push_str(&format!("\\u{:04x}", c as u32)),
                }
                continue;
            }
        } else if c == '"' {
            in_string = true;
        }
        out.push(c);
    }
    out
}

/// `,` before a closer, outside strings.
fn strip_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut in_string, mut escaped) = (false, false);
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if c == ',' {
            let next = chars[i + 1..].iter().find(|n| !n.is_whitespace());
            if matches!(next, Some('}' | ']')) {
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Backslashes JSON has no escape for (`\d`, `\s`, `\(`: regexes and Windows paths), doubled.
fn fix_invalid_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if !in_string {
            in_string = c == '"';
            out.push(c);
            continue;
        }
        match c {
            '"' => {
                in_string = false;
                out.push(c);
            }
            '\\' => match chars.peek() {
                Some('"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u') => {
                    out.push(c);
                    out.push(chars.next().expect("peeked"));
                }
                _ => out.push_str("\\\\"),
            },
            _ => out.push(c),
        }
    }
    out
}

enum Scan {
    InString,
    /// Closers still owed, in the order they were opened.
    Open(Vec<char>),
}

fn scan(text: &str) -> Scan {
    let mut stack = Vec::new();
    let (mut in_string, mut escaped) = (false, false);
    for c in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                if stack.last() == Some(&c) {
                    stack.pop();
                }
            }
            _ => {}
        }
    }
    if in_string {
        Scan::InString
    } else {
        Scan::Open(stack)
    }
}

/// The tool `name` was meant to be, from what models get wrong about names: case, `-` for `_`,
/// camelCase, `_tool` on the end, `functions.bash:3`, the name of the tool in another harness.
pub fn repair_tool_name(name: &str, known: &[&str]) -> Option<String> {
    if known.contains(&name) {
        return Some(name.to_string());
    }
    // A name running on into the arguments: `bash"}` or `read_file<arg>`.
    let cut = name.split(['"', '\'', '<', '>']).next().unwrap_or(name);
    let last = cut.rsplit('.').next().unwrap_or(cut);
    let last = last.split(':').next().unwrap_or(last).trim();
    let mut snake = String::with_capacity(last.len() + 4);
    for (i, c) in last.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 && !snake.ends_with('_') {
            snake.push('_');
        }
        match c {
            '-' | ' ' => snake.push('_'),
            c => snake.push(c.to_ascii_lowercase()),
        }
    }
    let mut candidates = vec![snake.clone()];
    let mut stripped = snake.as_str();
    for _ in 0..2 {
        match stripped.strip_suffix("_tool") {
            Some(s) => {
                stripped = s;
                candidates.push(s.to_string());
            }
            None => break,
        }
    }
    for candidate in &candidates {
        if known.contains(&candidate.as_str()) {
            return Some(candidate.clone());
        }
        let alias = match candidate.as_str() {
            "shell" | "sh" | "terminal" | "run" | "exec" | "execute" | "execute_command"
            | "run_command" | "run_shell_command" | "shell_command" | "run_terminal_cmd"
            | "command" => "bash",
            "read" | "view" | "cat" | "open_file" | "view_file" => "read_file",
            "write" | "create_file" | "create" | "save_file" => "write_file",
            "edit" | "str_replace" | "str_replace_editor" | "replace" | "replace_in_file"
            | "search_replace" | "apply_edit" => "edit_file",
            "search"
            | "rg"
            | "ripgrep"
            | "search_files"
            | "search_file_content"
            | "find_in_files" => "grep",
            "find" | "find_files" | "file_search" | "list_files_by_pattern" => "glob",
            "ls" | "list" | "list_files" | "list_directory" | "list_dir" | "dir" => "list_dir",
            "fetch" | "webfetch" | "web_get" | "fetch_url" | "get_url" => "web_fetch",
            _ => continue,
        };
        if known.contains(&alias) {
            return Some(alias.to_string());
        }
    }
    // Spelled nearly right.
    known
        .iter()
        .map(|k| (similarity(&snake, k), *k))
        .filter(|(score, _)| *score >= 0.7)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, k)| k.to_string())
}

/// How alike two names are: 1.0 for the same, by the edits between them.
fn similarity(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = if ca == cb {
                previous
            } else {
                1 + previous.min(here).min(row[j])
            };
            previous = here;
        }
    }
    1.0 - row[b.len()] as f64 / a.len().max(b.len()) as f64
}

/// `args` shaped to fit `parameters` (a tool's JSON schema): the field names other harnesses use,
/// numbers and booleans sent as strings, a value wrapped in an array, placeholder nulls. What it
/// can't fit is left for `missing_required` to tell the model about.
pub fn coerce_args(tool: &str, args: &mut Value, parameters: &Value) {
    let Value::Object(map) = args else { return };
    let properties = parameters["properties"].as_object();
    for (from, to) in aliases(tool) {
        if let Some(v) = map.remove(*from) {
            map.entry((*to).to_string()).or_insert(v);
        }
    }
    let Some(properties) = properties else { return };
    let required: HashSet<&str> = parameters["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let keys: Vec<String> = map.keys().cloned().collect();
    for key in keys {
        let Some(schema) = properties.get(&key) else {
            continue;
        };
        let value = map.get_mut(&key).expect("key from map");
        // A null or an empty placeholder for something optional means it wasn't given.
        let placeholder = value.is_null()
            || (value.as_str().is_some_and(|s| {
                let s = s.trim();
                s.is_empty() || s.eq_ignore_ascii_case("null") || s.eq_ignore_ascii_case("none")
            }) && schema["type"] != "string");
        if placeholder && !required.contains(key.as_str()) {
            map.remove(&key);
            continue;
        }
        coerce_value(value, schema);
    }
}

fn coerce_value(value: &mut Value, schema: &Value) {
    let Some(ty) = schema["type"].as_str() else {
        return;
    };
    match (ty, &*value) {
        ("integer" | "number", Value::String(s)) => {
            let s = s.trim();
            if let Ok(n) = s.parse::<i64>() {
                *value = json!(n);
            } else if let Ok(n) = s.parse::<f64>() {
                *value = if ty == "integer" && n.fract() == 0.0 {
                    json!(n as i64)
                } else {
                    json!(n)
                };
            }
        }
        ("integer", Value::Number(n)) => {
            if let Some(f) = n.as_f64().filter(|f| f.fract() == 0.0) {
                *value = json!(f as i64);
            }
        }
        ("boolean", Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" => *value = json!(true),
            "false" | "no" | "0" => *value = json!(false),
            _ => {}
        },
        ("boolean", Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                *value = json!(i != 0);
            }
        }
        // An object for `content` is a model writing a JSON file without encoding it. As anything
        // but text it would read as nothing, and write an empty file.
        ("string", Value::Number(_) | Value::Bool(_) | Value::Object(_) | Value::Array(_)) => {
            *value = json!(value.to_string());
        }
        ("array", Value::String(s)) => {
            *value = match serde_json::from_str::<Value>(s) {
                Ok(v @ Value::Array(_)) => v,
                _ => json!([s]),
            };
        }
        ("array", Value::Object(_) | Value::Number(_) | Value::Bool(_)) => {
            *value = json!([value.clone()]);
        }
        ("object", Value::String(s)) => {
            if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(s) {
                *value = v;
            }
        }
        _ => {}
    }
}

/// The field names other harnesses' tools use.
fn aliases(tool: &str) -> &'static [(&'static str, &'static str)] {
    match tool {
        "bash" => &[
            ("cmd", "command"),
            ("script", "command"),
            ("shell_command", "command"),
            ("timeout", "timeout_s"),
            ("timeout_seconds", "timeout_s"),
            ("run_in_background", "background"),
        ],
        "read_file" => &[
            ("file_path", "path"),
            ("filePath", "path"),
            ("filename", "path"),
            ("file", "path"),
            ("target_file", "path"),
            ("startLine", "start_line"),
            ("offset", "start_line"),
            ("from", "start_line"),
            ("endLine", "end_line"),
            ("to", "end_line"),
        ],
        "write_file" => &[
            ("file_path", "path"),
            ("filePath", "path"),
            ("filename", "path"),
            ("file", "path"),
            ("target_file", "path"),
            ("text", "content"),
            ("contents", "content"),
            ("file_text", "content"),
            ("data", "content"),
        ],
        "edit_file" => &[
            ("file_path", "path"),
            ("filePath", "path"),
            ("filename", "path"),
            ("file", "path"),
            ("target_file", "path"),
            ("old_string", "old_str"),
            ("oldString", "old_str"),
            ("old_text", "old_str"),
            ("oldText", "old_str"),
            ("search", "old_str"),
            ("find", "old_str"),
            ("new_string", "new_str"),
            ("newString", "new_str"),
            ("new_text", "new_str"),
            ("newText", "new_str"),
            ("replace", "new_str"),
            ("replacement", "new_str"),
            ("replaceAll", "replace_all"),
        ],
        "grep" => &[
            ("query", "pattern"),
            ("regex", "pattern"),
            ("search", "pattern"),
            ("dir", "path"),
            ("directory", "path"),
            ("include", "glob"),
            ("file_pattern", "glob"),
            ("ignore_case", "case_insensitive"),
            ("-i", "case_insensitive"),
        ],
        "glob" => &[
            ("query", "pattern"),
            ("glob", "pattern"),
            ("dir", "path"),
            ("directory", "path"),
        ],
        "list_dir" => &[
            ("dir", "path"),
            ("directory", "path"),
            ("dir_path", "path"),
            ("relative_workspace_path", "path"),
        ],
        "web_fetch" => &[("uri", "url"), ("link", "url")],
        _ => &[],
    }
}

/// Which required fields `args` lacks, by `parameters`: running without them would write an empty
/// file or delete the text being replaced. An empty string counts as given.
pub fn missing_required(args: &Value, parameters: &Value) -> Vec<String> {
    let empty = Map::new();
    let map = args.as_object().unwrap_or(&empty);
    parameters["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|k| map.get(*k).is_none_or(Value::is_null))
        .map(String::from)
        .collect()
}

/// What the model is told about arguments that couldn't be read, for it to send them right.
pub fn invalid_args_message(tool: &str, raw: &str, why: &str) -> String {
    let shown: String = raw.chars().take(300).collect();
    format!(
        "Error: this call to {tool} was not run because its arguments aren't valid JSON ({why}). \
         They began: {shown}\nCall {tool} again with a single JSON object as its arguments."
    )
}

/// What the model is told about a call the output limit cut off.
pub fn truncated_args_message(tool: &str) -> String {
    format!(
        "Error: this call to {tool} was not run because it was cut off before it finished (the \
         answer hit its output limit). Nothing was written or executed. Make it smaller: split a \
         large file into several writes, or send the change as a few edit_file calls."
    )
}

/// `calls` with ids every provider takes: none empty, none repeated, no characters a strict
/// upstream refuses.
pub fn normalize_ids(
    calls: &mut [AccumulatedToolCall],
    seen: &mut HashSet<String>,
    mint: impl Fn(usize) -> String,
) {
    for (i, call) in calls.iter_mut().enumerate() {
        let clean: String = call
            .id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(64)
            .collect();
        call.id = if clean.is_empty() || !seen.insert(clean.clone()) {
            let id = mint(i);
            seen.insert(id.clone());
            id
        } else {
            clean
        };
    }
}

/// Tool calls a model wrote as text, in the formats model families use when their provider's
/// template doesn't turn them into structured calls: `(calls, the text without them)`. Only for an
/// answer with no structured calls, and only names in `known` count. A call is the last thing in
/// an answer, outside any code block: a model explaining a format, or quoting a command, is showing
/// it, and running what it shows could do real harm.
pub fn parse_text_calls(text: &str, known: &[&str]) -> (Vec<(String, String)>, String) {
    let (before, tail) = after_last_fence(text);
    let mut calls = Vec::new();
    let mut rest = tail.to_string();
    if ends_with_a_call(tail) {
        for parser in [
            dsml_calls as fn(&str, &[&str], &mut Vec<(String, String)>) -> String,
            kimi_calls,
            tagged_calls,
            function_tag_calls,
        ] {
            rest = parser(&rest, known, &mut calls);
        }
    }
    if calls.is_empty() {
        return match bare_json_call(text, known) {
            Some((call, without)) => (vec![call], without),
            None => (Vec::new(), text.trim().to_string()),
        };
    }
    (calls, format!("{before}{rest}").trim().to_string())
}

/// `text` split after its last closed code fence. Inside an open fence, nothing comes after.
fn after_last_fence(text: &str) -> (&str, &str) {
    let fences: Vec<usize> = text.match_indices("```").map(|(i, _)| i).collect();
    match fences.last() {
        None => ("", text),
        Some(_) if fences.len() % 2 == 1 => (text, ""),
        Some(last) => text.split_at(last + 3),
    }
}

fn ends_with_a_call(text: &str) -> bool {
    let end = text.trim_end().replace('｜', "|");
    [
        "</tool_call>",
        "</function>",
        "<|tool_call_end|>",
        "<|tool_call_argument_end|>",
        "<|tool_calls_section_end|>",
        "</|DSML|invoke>",
        "</|DSML|function_calls>",
    ]
    .iter()
    .any(|closer| end.ends_with(closer))
}

fn known_name(name: &str, known: &[&str]) -> Option<String> {
    let name = name.trim();
    (!name.is_empty())
        .then(|| repair_tool_name(name, known))
        .flatten()
}

/// `<tool_call>…</tool_call>`, whose body is JSON (`{"name":…,"arguments":…}`), a
/// `<function=NAME>` form, or GLM's `NAME<arg_key>k</arg_key><arg_value>v</arg_value>`.
fn tagged_calls(text: &str, known: &[&str], calls: &mut Vec<(String, String)>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<tool_call>") {
        let after = &rest[start + "<tool_call>".len()..];
        let Some(end) = after.find("</tool_call>") else {
            break;
        };
        let body = after[..end].trim();
        let parsed = call_from_body(body, known);
        if parsed.is_none() {
            out.push_str(&rest[..start + "<tool_call>".len() + end + "</tool_call>".len()]);
        } else {
            out.push_str(&rest[..start]);
            calls.extend(parsed);
        }
        rest = &after[end + "</tool_call>".len()..];
    }
    out.push_str(rest);
    out
}

fn call_from_body(body: &str, known: &[&str]) -> Option<(String, String)> {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        return json_call(&v, known);
    }
    if body.starts_with("<function=") {
        let mut found = Vec::new();
        let rest = function_tag_calls(body, known, &mut found);
        if rest.trim().is_empty() {
            return found.pop();
        }
        // `<function=name>{json}` without its closing tag.
        let after = body.strip_prefix("<function=")?;
        let (name, args) = after.split_once('>')?;
        let name = known_name(name, known)?;
        let args = args.trim().trim_end_matches("</function>").trim();
        return Some((name, args.to_string()));
    }
    if let Some(open) = body.find("<arg_key>") {
        let name = known_name(&body[..open], known)?;
        let mut map = Map::new();
        let mut rest = &body[open..];
        while let Some(start) = rest.find("<arg_key>") {
            let tail = &rest[start + "<arg_key>".len()..];
            let close = tail.find("</arg_key>")?;
            let key = tail[..close].trim().to_string();
            let tail = &tail[close + "</arg_key>".len()..];
            let open_value = tail.find("<arg_value>")?;
            let tail = &tail[open_value + "<arg_value>".len()..];
            let close_value = tail.find("</arg_value>")?;
            map.insert(key, scalar(&tail[..close_value]));
            rest = &tail[close_value + "</arg_value>".len()..];
        }
        return Some((name, Value::Object(map).to_string()));
    }
    None
}

/// A parameter written as text: JSON when it is (numbers, arrays), else the string.
fn scalar(raw: &str) -> Value {
    let trimmed = raw.trim();
    match serde_json::from_str::<Value>(trimmed) {
        Ok(v @ (Value::Number(_) | Value::Bool(_) | Value::Array(_) | Value::Object(_))) => v,
        _ => Value::String(raw.trim_matches('\n').to_string()),
    }
}

fn json_call(v: &Value, known: &[&str]) -> Option<(String, String)> {
    let f = if v.get("function").is_some_and(Value::is_object) {
        &v["function"]
    } else {
        v
    };
    let name = known_name(f["name"].as_str()?, known)?;
    let args = f.get("arguments").or_else(|| f.get("parameters"));
    let args = match args {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => "{}".to_string(),
    };
    Some((name, args))
}

/// Qwen's `<function=NAME><parameter=KEY>VALUE</parameter></function>`.
fn function_tag_calls(text: &str, known: &[&str], calls: &mut Vec<(String, String)>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<function=") {
        let after = &rest[start + "<function=".len()..];
        let parsed = after.find('>').and_then(|close_name| {
            let end = after.find("</function>")?;
            if close_name >= end {
                return None;
            }
            let name = known_name(&after[..close_name], known)?;
            let body = &after[close_name + 1..end];
            let mut map = Map::new();
            let mut params = body;
            while let Some(p) = params.find("<parameter=") {
                let tail = &params[p + "<parameter=".len()..];
                let key_end = tail.find('>')?;
                let key = tail[..key_end].trim().to_string();
                let tail = &tail[key_end + 1..];
                let value_end = tail.find("</parameter>")?;
                map.insert(key, scalar(&tail[..value_end]));
                params = &tail[value_end + "</parameter>".len()..];
            }
            let args = if map.is_empty() && body.trim().starts_with('{') {
                body.trim().to_string()
            } else {
                Value::Object(map).to_string()
            };
            Some(((name, args), end + "</function>".len()))
        });
        match parsed {
            Some((call, consumed)) => {
                out.push_str(&rest[..start]);
                calls.push(call);
                rest = &after[consumed..];
            }
            None => {
                out.push_str(&rest[..start + "<function=".len()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Kimi's `<|tool_calls_section_begin|><|tool_call_begin|>functions.NAME:0<|tool_call_argument_begin|>{json}<|tool_call_end|><|tool_calls_section_end|>`.
fn kimi_calls(text: &str, known: &[&str], calls: &mut Vec<(String, String)>) -> String {
    const CALL: &str = "<|tool_call_begin|>";
    const ARGS: &str = "<|tool_call_argument_begin|>";
    const END: &str = "<|tool_call_end|>";
    if !text.contains(CALL) {
        return text.to_string();
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(CALL) {
        out.push_str(&rest[..start]);
        let after = &rest[start + CALL.len()..];
        let Some(args_at) = after.find(ARGS) else {
            rest = after;
            continue;
        };
        let tail = &after[args_at + ARGS.len()..];
        let end = tail
            .find(END)
            .or_else(|| tail.find("<|tool_call_argument_end|>"))
            .unwrap_or(tail.len());
        if let Some(name) = known_name(&after[..args_at], known) {
            calls.push((name, tail[..end].trim().to_string()));
        }
        rest = &tail[end..];
        rest = rest
            .strip_prefix("<|tool_call_argument_end|>")
            .unwrap_or(rest);
        rest = rest.strip_prefix(END).unwrap_or(rest);
    }
    out.push_str(rest);
    out.replace("<|tool_calls_section_begin|>", "")
        .replace("<|tool_calls_section_end|>", "")
}

/// DeepSeek's `<｜DSML｜invoke name="x"><｜DSML｜parameter name="k" string="true">v</｜DSML｜parameter></｜DSML｜invoke>`,
/// with an ASCII or a full-width bar.
fn dsml_calls(text: &str, known: &[&str], calls: &mut Vec<(String, String)>) -> String {
    if !text.contains("DSML") {
        return text.to_string();
    }
    let text = text.replace('｜', "|");
    const INVOKE: &str = "<|DSML|invoke";
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find(INVOKE) {
        out.push_str(&rest[..start]);
        let after = &rest[start + INVOKE.len()..];
        let Some(end) = after.find("</|DSML|invoke>") else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let body = &after[..end];
        if let Some(name) = attr(body, "name").and_then(|n| known_name(&n, known)) {
            let mut map = Map::new();
            let mut params = body;
            while let Some(p) = params.find("<|DSML|parameter") {
                let tail = &params[p + "<|DSML|parameter".len()..];
                let Some(open_end) = tail.find('>') else {
                    break;
                };
                let Some(close) = tail.find("</|DSML|parameter>") else {
                    break;
                };
                if open_end >= close {
                    break;
                }
                if let Some(key) = attr(&tail[..open_end], "name") {
                    let raw = &tail[open_end + 1..close];
                    let is_string = attr(&tail[..open_end], "string").as_deref() == Some("true");
                    let value = if is_string {
                        Value::String(raw.to_string())
                    } else {
                        scalar(raw)
                    };
                    map.insert(key, value);
                }
                params = &tail[close + "</|DSML|parameter>".len()..];
            }
            calls.push((name, Value::Object(map).to_string()));
        }
        rest = &after[end + "</|DSML|invoke>".len()..];
    }
    out.push_str(rest);
    // The wrapper around the calls: `<|DSML|function_calls>…</|DSML|function_calls>`.
    out.replace("<|DSML|function_calls>", "")
        .replace("</|DSML|function_calls>", "")
}

fn attr(text: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let start = text.find(&key)? + key.len();
    let end = text[start..].find('"')?;
    Some(text[start..start + end].to_string())
}

/// A whole answer that is a call: `{"name":"bash","arguments":{…}}`, optionally in a fence. Inside
/// other text it stays text: models show JSON all the time.
fn bare_json_call(text: &str, known: &[&str]) -> Option<((String, String), String)> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|b| b.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    if !body.starts_with('{') {
        return None;
    }
    let v: Value = serde_json::from_str(body).ok()?;
    v.get("arguments").or_else(|| v.get("parameters"))?;
    Some((json_call(&v, known)?, String::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: &[&str] = &[
        "bash",
        "read_file",
        "write_file",
        "edit_file",
        "grep",
        "glob",
        "list_dir",
    ];

    #[test]
    fn clean_arguments_pass_through() {
        assert_eq!(
            parse_args(r#"{"command":"ls"}"#),
            Args::Clean(json!({"command":"ls"}))
        );
        assert_eq!(parse_args(""), Args::Clean(json!({})));
        assert_eq!(parse_args("   "), Args::Clean(json!({})));
    }

    #[test]
    fn a_second_value_after_the_arguments_is_dropped() {
        // A real MiMo call: the path was there, and the old message said it wasn't.
        assert_eq!(
            parse_args(r#"{"path":"/tmp/x.md"}{}"#),
            Args::Repaired(json!({"path":"/tmp/x.md"}))
        );
    }

    #[test]
    fn arguments_encoded_twice_are_unwrapped() {
        assert_eq!(
            parse_args(r#""{\"command\":\"ls\"}""#),
            Args::Repaired(json!({"command":"ls"}))
        );
    }

    #[test]
    fn raw_newlines_in_strings_are_repaired() {
        assert_eq!(
            parse_args("{\"content\":\"line 1\nline 2\t!\"}"),
            Args::Repaired(json!({"content":"line 1\nline 2\t!"}))
        );
    }

    #[test]
    fn trailing_commas_are_repaired_but_commas_in_strings_stay() {
        assert_eq!(
            parse_args(r#"{"a":"x,}","b":[1,2,],}"#),
            Args::Repaired(json!({"a":"x,}","b":[1,2]}))
        );
    }

    #[test]
    fn invalid_escapes_are_doubled() {
        assert_eq!(
            parse_args(r#"{"pattern":"\d+\s","path":"a\\b"}"#),
            Args::Repaired(json!({"pattern":"\\d+\\s","path":"a\\b"}))
        );
    }

    #[test]
    fn text_cut_off_is_never_closed_and_run() {
        assert_eq!(parse_args(r#"{"command":"rm -rf /tmp/bu"#), Args::Truncated);
        assert_eq!(
            parse_args(r#"{"path":"a","content":"line"#),
            Args::Truncated
        );
        assert_eq!(parse_args(r#"{"a":1,"#), Args::Truncated);
    }

    #[test]
    fn missing_closers_after_a_finished_value_are_added() {
        assert_eq!(
            parse_args(r#"{"a":{"b":[1,2]}"#),
            Args::Repaired(json!({"a":{"b":[1,2]}}))
        );
    }

    #[test]
    fn excess_closers_are_dropped() {
        assert_eq!(parse_args(r#"{"a":1}}}"#), Args::Repaired(json!({"a":1})),);
    }

    #[test]
    fn what_is_not_an_object_says_so() {
        assert!(matches!(parse_args("[1,2]"), Args::Invalid(_)));
        assert!(matches!(parse_args("42"), Args::Invalid(_)));
        assert!(matches!(parse_args("not json at all}"), Args::Invalid(_)));
    }

    #[test]
    fn tool_names_are_repaired_from_common_mistakes() {
        for (given, want) in [
            ("bash", "bash"),
            ("Bash", "bash"),
            ("shell", "bash"),
            ("functions.bash:3", "bash"),
            ("read-file", "read_file"),
            ("readFile", "read_file"),
            ("read_file_tool", "read_file"),
            ("Read", "read_file"),
            ("str_replace", "edit_file"),
            ("edit_fil", "edit_file"),
            ("bash\"}", "bash"),
            ("rg", "grep"),
            ("ls", "list_dir"),
            ("default_api.write_file", "write_file"),
        ] {
            assert_eq!(
                repair_tool_name(given, KNOWN).as_deref(),
                Some(want),
                "{given}"
            );
        }
        assert_eq!(repair_tool_name("deploy_to_prod", KNOWN), None);
        assert_eq!(repair_tool_name("", KNOWN), None);
    }

    #[test]
    fn an_alias_only_counts_for_a_tool_the_session_has() {
        assert_eq!(repair_tool_name("fetch", KNOWN), None);
        assert_eq!(
            repair_tool_name("fetch", &["web_fetch"]).as_deref(),
            Some("web_fetch")
        );
    }

    #[test]
    fn field_names_and_types_are_coerced_to_the_schema() {
        let schema = json!({
            "properties": {
                "path": {"type":"string"},
                "start_line": {"type":"integer"},
                "replace_all": {"type":"boolean"},
                "old_str": {"type":"string"},
            },
            "required": ["path"]
        });
        let mut args = json!({
            "file_path": "a.rs",
            "start_line": "12",
            "replaceAll": "true",
            "old_string": "x",
        });
        coerce_args("edit_file", &mut args, &schema);
        assert_eq!(
            args,
            json!({"path":"a.rs","start_line":12,"replace_all":true,"old_str":"x"})
        );
    }

    #[test]
    fn placeholder_nulls_for_optional_fields_go_away() {
        let schema = json!({
            "properties": {"path":{"type":"string"},"end_line":{"type":"integer"},"start_line":{"type":"integer"}},
            "required": ["path"]
        });
        let mut args = json!({"path":"a","end_line":null,"start_line":"null"});
        coerce_args("read_file", &mut args, &schema);
        assert_eq!(args, json!({"path":"a"}));
    }

    #[test]
    fn a_string_field_keeps_an_empty_string() {
        let schema = json!({"properties":{"new_str":{"type":"string"}},"required":["new_str"]});
        let mut args = json!({"new_str":""});
        coerce_args("edit_file", &mut args, &schema);
        assert_eq!(args, json!({"new_str":""}));
        assert!(missing_required(&args, &schema).is_empty());
    }

    #[test]
    fn required_fields_that_are_missing_are_named() {
        let schema = json!({"required":["path","content"]});
        assert_eq!(
            missing_required(&json!({"path":"a"}), &schema),
            vec!["content"]
        );
        assert_eq!(
            missing_required(&json!({"path":null,"content":""}), &schema),
            vec!["path"]
        );
    }

    #[test]
    fn ids_end_up_unique_and_plain() {
        let mut calls: Vec<AccumulatedToolCall> = ["a", "a", "", "fc.1:x"]
            .iter()
            .map(|id| AccumulatedToolCall {
                id: id.to_string(),
                ..Default::default()
            })
            .collect();
        let mut seen = HashSet::new();
        normalize_ids(&mut calls, &mut seen, |i| format!("minted_{i}"));
        let ids: Vec<&str> = calls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "minted_1", "minted_2", "fc1x"]);
        // An id from an earlier answer isn't used again.
        let mut next = vec![AccumulatedToolCall {
            id: "a".into(),
            ..Default::default()
        }];
        normalize_ids(&mut next, &mut seen, |_| "fresh".into());
        assert_eq!(next[0].id, "fresh");
    }

    fn text_call(text: &str) -> (String, Value, String) {
        let (mut calls, rest) = parse_text_calls(text, KNOWN);
        assert_eq!(calls.len(), 1, "{text}");
        let (name, args) = calls.pop().unwrap();
        (name, serde_json::from_str(&args).unwrap(), rest)
    }

    #[test]
    fn text_calls_in_each_house_format_are_read() {
        let (name, args, rest) = text_call(
            "Running.\n<tool_call>{\"name\":\"bash\",\"arguments\":{\"command\":\"ls\"}}</tool_call>",
        );
        assert_eq!((name.as_str(), &args), ("bash", &json!({"command":"ls"})));
        assert_eq!(rest, "Running.");

        let (name, args, _) = text_call(
            "<function=read_file><parameter=path>a.rs</parameter><parameter=start_line>3</parameter></function>",
        );
        assert_eq!(name, "read_file");
        assert_eq!(args, json!({"path":"a.rs","start_line":3}));

        let (name, args, _) = text_call(
            "<tool_call><function=read_file><parameter=path>a.rs</parameter></function></tool_call>",
        );
        assert_eq!(name, "read_file");
        assert_eq!(args, json!({"path":"a.rs"}));

        let (name, args, _) = text_call(
            "<|tool_calls_section_begin|><|tool_call_begin|>functions.bash:0<|tool_call_argument_begin|>{\"command\":\"ls\"}<|tool_call_end|><|tool_calls_section_end|>",
        );
        assert_eq!(name, "bash");
        assert_eq!(args, json!({"command":"ls"}));

        let (name, args, _) = text_call(
            "<tool_call>read_file<arg_key>path</arg_key><arg_value>a.rs</arg_value></tool_call>",
        );
        assert_eq!(name, "read_file");
        assert_eq!(args, json!({"path":"a.rs"}));

        for bar in ["|", "｜"] {
            let (name, args, _) = text_call(&format!(
                "<{bar}DSML{bar}function_calls><{bar}DSML{bar}invoke name=\"bash\"><{bar}DSML{bar}parameter name=\"command\" string=\"true\">ls -a</{bar}DSML{bar}parameter></{bar}DSML{bar}invoke></{bar}DSML{bar}function_calls>"
            ));
            assert_eq!(name, "bash");
            assert_eq!(args, json!({"command":"ls -a"}));
        }

        let (name, args, rest) =
            text_call("```json\n{\"name\":\"bash\",\"arguments\":{\"command\":\"ls\"}}\n```");
        assert_eq!(name, "bash");
        assert_eq!(args, json!({"command":"ls"}));
        assert_eq!(rest, "");
    }

    #[test]
    fn malformed_text_calls_dont_panic() {
        for text in [
            "<function=bash</function>",
            "<|DSML|invoke name=\"bash\"><|DSML|parameter name=\"x\"</|DSML|parameter></|DSML|invoke>",
            "<tool_call></tool_call>",
            "<tool_call>bash<arg_key>x</tool_call>",
            "<|tool_call_begin|><|tool_call_end|>",
        ] {
            let _ = parse_text_calls(text, KNOWN);
        }
    }

    #[test]
    fn non_text_for_a_text_field_becomes_text() {
        let schema = json!({ "properties": { "content": { "type": "string" } } });
        let mut args = json!({ "content": { "name": "x" } });
        coerce_args("write_file", &mut args, &schema);
        assert_eq!(args["content"], "{\"name\":\"x\"}");
        let mut args = json!({ "content": [1, 2] });
        coerce_args("write_file", &mut args, &schema);
        assert_eq!(args["content"], "[1,2]");
    }

    #[test]
    fn an_empty_value_before_the_real_one_is_passed_over() {
        assert_eq!(
            parse_args(r#"{}{"command":"ls"}"#),
            Args::Repaired(json!({"command":"ls"}))
        );
        assert_eq!(parse_args("{}{}"), Args::Repaired(json!({})));
    }

    #[test]
    fn a_call_being_shown_isnt_run() {
        let danger =
            r#"<tool_call>{"name":"bash","arguments":{"command":"rm -rf build"}}</tool_call>"#;
        for text in [
            format!("The format looks like this:\n```\n{danger}\n```\nThat's how."),
            format!("The format looks like this:\n```\n{danger}\n```"),
            format!("An open block:\n```\n{danger}"),
            format!("Write {danger} to call it, and then wait."),
            format!("Like `{danger}`"),
        ] {
            let (calls, rest) = parse_text_calls(&text, KNOWN);
            assert!(calls.is_empty(), "{text}");
            assert_eq!(rest, text.trim());
        }
        // Words and code before it are fine: the call is the last thing said.
        let (calls, rest) = parse_text_calls(
            &format!("First:\n```\nsome code\n```\nNow.\n{danger}"),
            KNOWN,
        );
        assert_eq!(calls.len(), 1);
        assert!(
            rest.contains("some code") && rest.ends_with("Now."),
            "{rest}"
        );
    }

    #[test]
    fn prose_about_tools_stays_prose() {
        for text in [
            "To run it, call the bash tool with {\"command\": \"ls\"}.",
            "Here is the format: <tool_call>{\"name\":\"launch_rocket\",\"arguments\":{}}</tool_call>",
            "{\"name\":\"bash\",\"version\":\"5\"}",
        ] {
            let (calls, rest) = parse_text_calls(text, KNOWN);
            assert!(calls.is_empty(), "{text}");
            assert_eq!(rest, text.trim());
        }
    }
}
