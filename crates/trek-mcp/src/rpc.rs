//! JSON-RPC 2.0 / MCP dispatch, independent of the tool family.

use serde_json::{Value, json};

pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
pub const LATEST_PROTOCOL_VERSION: &str = "2025-06-18";

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

/// Result of a tool call: MCP content blocks, or an error message that is
/// reported to the model as `isError: true`.
pub type ToolResult = Result<Vec<Value>, String>;

/// One MCP tool definition.
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

impl ToolDef {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
        })
    }
}

/// A family of tools served by one `trek-mcp <family>` process.
pub trait ToolSet {
    fn family(&self) -> &'static str;
    fn instructions(&self) -> &'static str;
    fn tools(&self) -> Vec<ToolDef>;
    /// Run a tool. Only called with names returned by [`ToolSet::tools`].
    fn call(&mut self, name: &str, args: &Value) -> ToolResult;
}

pub fn text(s: impl Into<String>) -> Value {
    json!({ "type": "text", "text": s.into() })
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn image_png(base64: String) -> Value {
    json!({ "type": "image", "data": base64, "mimeType": "image/png" })
}

#[derive(Default)]
pub struct Server {
    pub initialized: bool,
    pub protocol_version: Option<String>,
}

impl Server {
    pub fn new() -> Self {
        Self::default()
    }

    /// Handle one line of input. Returns the serialized response line, if any
    /// (notifications produce no response).
    pub fn handle_line(&mut self, line: &str, tools: &mut dyn ToolSet) -> Option<String> {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(error_response(Value::Null, PARSE_ERROR, &format!("Parse error: {e}")).to_string());
            }
        };
        match msg {
            // JSON-RPC batch (allowed by protocol 2025-03-26).
            Value::Array(items) => {
                if items.is_empty() {
                    return Some(error_response(Value::Null, INVALID_REQUEST, "Empty batch").to_string());
                }
                let responses: Vec<Value> = items
                    .into_iter()
                    .filter_map(|m| self.handle_message(m, tools))
                    .collect();
                if responses.is_empty() {
                    None
                } else {
                    Some(Value::Array(responses).to_string())
                }
            }
            other => self.handle_message(other, tools).map(|v| v.to_string()),
        }
    }

    pub fn handle_message(&mut self, msg: Value, tools: &mut dyn ToolSet) -> Option<Value> {
        let Some(obj) = msg.as_object() else {
            return Some(error_response(Value::Null, INVALID_REQUEST, "Request must be an object"));
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // A response from the client (we never send requests) or garbage.
            if obj.contains_key("result") || obj.contains_key("error") {
                return None;
            }
            return Some(error_response(id.unwrap_or(Value::Null), INVALID_REQUEST, "Missing method"));
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);

        // Notifications: no id, never answered.
        let Some(id) = id else {
            match method {
                "notifications/initialized" => self.initialized = true,
                "notifications/cancelled" => {}
                other => eprintln!("trek-mcp: ignoring notification {other}"),
            }
            return None;
        };

        let outcome: Result<Value, (i64, String)> = match method {
            "initialize" => Ok(self.initialize(&params, tools)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({
                "tools": tools.tools().iter().map(ToolDef::to_json).collect::<Vec<_>>()
            })),
            "tools/call" => self.call_tool(&params, tools),
            // Answer the optional list methods with empty sets so clients that
            // probe them regardless of capabilities don't log errors.
            "resources/list" => Ok(json!({ "resources": [] })),
            "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            other => Err((METHOD_NOT_FOUND, format!("Method not found: {other}"))),
        };
        Some(match outcome {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => error_response(id, code, &message),
        })
    }

    fn initialize(&mut self, params: &Value, tools: &dyn ToolSet) -> Value {
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        let version = negotiate_version(requested);
        self.protocol_version = Some(version.to_string());
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "trek",
                "title": format!("Trek {}", tools.family()),
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": tools.instructions(),
        })
    }

    fn call_tool(&mut self, params: &Value, tools: &mut dyn ToolSet) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or((INVALID_PARAMS, "tools/call requires params.name".to_string()))?;
        if !tools.tools().iter().any(|t| t.name == name) {
            return Err((INVALID_PARAMS, format!("Unknown tool: {name}")));
        }
        let args = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => return Err((INVALID_PARAMS, "params.arguments must be an object".to_string())),
        };
        eprintln!("trek-mcp: tools/call {name}");
        Ok(match tools.call(name, &args) {
            Ok(content) => json!({ "content": content }),
            Err(message) => {
                eprintln!("trek-mcp: {name} failed: {message}");
                json!({ "content": [text(message)], "isError": true })
            }
        })
    }
}

pub fn negotiate_version(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|r| SUPPORTED_PROTOCOL_VERSIONS.iter().find(|v| **v == r).copied())
        .unwrap_or(LATEST_PROTOCOL_VERSION)
}

pub fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

// ---- argument helpers shared by tool families ----
// Only the macOS families take points and images today; Windows computer use (Phase 4) will.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]

pub fn arg_f64(args: &Value, key: &str) -> Result<f64, String> {
    args.get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("Missing or non-numeric argument `{key}`"))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn opt_f64(args: &Value, key: &str) -> Result<Option<f64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().map(Some).ok_or_else(|| format!("Argument `{key}` must be a number")),
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Missing or non-string argument `{key}`"))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

/// Read `{x, y}` from `args[key]`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn arg_point(args: &Value, key: &str) -> Result<(f64, f64), String> {
    let p = args
        .get(key)
        .ok_or_else(|| format!("Missing argument `{key}` ({{x, y}})"))?;
    Ok((
        arg_f64(p, "x").map_err(|e| format!("{key}: {e}"))?,
        arg_f64(p, "y").map_err(|e| format!("{key}: {e}"))?,
    ))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn point_schema(desc: &str) -> Value {
    json!({
        "type": "object",
        "description": desc,
        "properties": { "x": { "type": "number" }, "y": { "type": "number" } },
        "required": ["x", "y"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;
    impl ToolSet for Echo {
        fn family(&self) -> &'static str {
            "test"
        }
        fn instructions(&self) -> &'static str {
            "test tools"
        }
        fn tools(&self) -> Vec<ToolDef> {
            vec![
                ToolDef {
                    name: "echo",
                    description: "Echo `text` back",
                    input_schema: json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}),
                },
                ToolDef {
                    name: "fail",
                    description: "Always fails",
                    input_schema: json!({"type":"object","properties":{}}),
                },
            ]
        }
        fn call(&mut self, name: &str, args: &Value) -> ToolResult {
            match name {
                "echo" => Ok(vec![text(arg_str(args, "text")?)]),
                _ => Err("boom".into()),
            }
        }
    }

    fn roundtrip(server: &mut Server, line: &str) -> Option<Value> {
        server
            .handle_line(line, &mut Echo)
            .map(|s| serde_json::from_str(&s).expect("server emits valid JSON"))
    }

    #[test]
    fn initialize_echoes_supported_version() {
        let mut s = Server::new();
        for v in SUPPORTED_PROTOCOL_VERSIONS {
            let line = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":v,"capabilities":{},"clientInfo":{"name":"t","version":"0"}}});
            let r = roundtrip(&mut s, &line.to_string()).unwrap();
            assert_eq!(r["id"], 1);
            assert_eq!(r["result"]["protocolVersion"], *v);
            assert_eq!(r["result"]["serverInfo"]["name"], "trek");
            assert!(r["result"]["capabilities"]["tools"].is_object());
        }
    }

    #[test]
    fn initialize_falls_back_to_latest() {
        let mut s = Server::new();
        let r = roundtrip(
            &mut s,
            r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        )
        .unwrap();
        assert_eq!(r["id"], "a");
        assert_eq!(r["result"]["protocolVersion"], LATEST_PROTOCOL_VERSION);
    }

    #[test]
    fn notifications_get_no_response() {
        let mut s = Server::new();
        assert!(roundtrip(&mut s, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        assert!(s.initialized);
        assert!(roundtrip(&mut s, r#"{"jsonrpc":"2.0","method":"notifications/whatever"}"#).is_none());
    }

    #[test]
    fn ping_and_unknown_method() {
        let mut s = Server::new();
        let r = roundtrip(&mut s, r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#).unwrap();
        assert_eq!(r["result"], json!({}));
        let r = roundtrip(&mut s, r#"{"jsonrpc":"2.0","id":8,"method":"sampling/createMessage"}"#).unwrap();
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(r["id"], 8);
    }

    #[test]
    fn parse_error() {
        let mut s = Server::new();
        let r = roundtrip(&mut s, "{not json").unwrap();
        assert_eq!(r["error"]["code"], PARSE_ERROR);
        assert_eq!(r["id"], Value::Null);
    }

    #[test]
    fn tools_list_has_schemas() {
        let mut s = Server::new();
        let r = roundtrip(&mut s, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        let tools = r["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "echo");
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn tools_call_success_error_and_unknown() {
        let mut s = Server::new();
        let r = roundtrip(
            &mut s,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"text":"hi"}}}"#,
        )
        .unwrap();
        assert_eq!(r["result"]["content"][0], json!({"type":"text","text":"hi"}));
        assert!(r["result"].get("isError").is_none());

        let r = roundtrip(
            &mut s,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{}}}"#,
        )
        .unwrap();
        assert_eq!(r["result"]["isError"], true);

        let r = roundtrip(&mut s, r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"fail"}}"#).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(r["result"]["content"][0]["text"], "boom");

        let r = roundtrip(&mut s, r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"nope"}}"#).unwrap();
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn batch_requests() {
        let mut s = Server::new();
        let r = roundtrip(
            &mut s,
            r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","id":2,"method":"nope"}]"#,
        )
        .unwrap();
        let arr = r.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], 1);
        assert_eq!(arr[1]["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn client_responses_are_ignored() {
        let mut s = Server::new();
        assert!(roundtrip(&mut s, r#"{"jsonrpc":"2.0","id":9,"result":{}}"#).is_none());
    }
}
