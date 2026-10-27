//! `trek-mcp browser`: browser use in Trek's own Browser tool. Every call is passed to the Trek
//! that started the session over its local channel (`trek_ipc`, as `orchestrate` does); Trek
//! drives the page the user watches and decides what needs their approval. See
//! `trek_ipc::browser` for the tools and what comes back.

use crate::rpc::{self, ToolDef, ToolSet};
use serde_json::Value;

pub struct Browser {
    client: Option<trek_ipc::Client>,
}

impl Browser {
    pub fn from_env() -> Self {
        Self {
            client: trek_ipc::Client::from_env(),
        }
    }
}

impl ToolSet for Browser {
    fn family(&self) -> &'static str {
        "browser"
    }

    fn instructions(&self) -> &'static str {
        trek_ipc::browser::INSTRUCTIONS
    }

    fn tools(&self) -> Vec<ToolDef> {
        trek_ipc::browser::tools()
            .into_iter()
            .map(|(name, description, input_schema)| ToolDef {
                name,
                description,
                input_schema,
            })
            .collect()
    }

    fn call(&mut self, name: &str, args: &Value) -> rpc::ToolResult {
        let client = self.client.as_ref().ok_or(
            "The browser tools work only in agents that Trek started (Trek's connection details are missing).",
        )?;
        let result = client.call(name, args)?;
        Ok(content(result))
    }
}

/// What Trek answered, as MCP content: its text, and the picture it wrote to a file (read, then
/// removed: it was made for this call alone).
pub(crate) fn content(result: Value) -> Vec<Value> {
    let mut out = vec![];
    if let Some(path) = result.get("image").and_then(Value::as_str) {
        use base64::Engine as _;
        match std::fs::read(path) {
            Ok(bytes) => {
                out.push(rpc::image_png(
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                ));
            }
            Err(e) => out.push(rpc::text(format!("(The screenshot couldn't be read: {e})"))),
        }
        let _ = std::fs::remove_file(path);
    }
    let text = match &result {
        Value::String(s) => s.clone(),
        Value::Object(o) => match o.get("text") {
            Some(Value::String(s)) => s.clone(),
            _ => serde_json::to_string_pretty(&result).unwrap_or_default(),
        },
        other => other.to_string(),
    };
    if !text.is_empty() {
        out.push(rpc::text(text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lists_the_shared_tools_with_their_instructions() {
        let tools = Browser { client: None };
        let names: Vec<&str> = tools.tools().iter().map(|t| t.name).collect();
        assert_eq!(names, trek_ipc::browser::TOOL_NAMES);
        assert!(tools.instructions().contains("browser_snapshot"));
    }

    #[test]
    fn without_trek_calls_say_why() {
        let mut tools = Browser { client: None };
        let err = tools
            .call("browser_open", &json!({"url": "example.com"}))
            .unwrap_err();
        assert!(err.contains("only in agents that Trek started"), "{err}");
    }

    #[test]
    fn a_picture_comes_from_its_file_which_goes() {
        let path = std::env::temp_dir().join(format!("trek-mcp-shot-{}.png", std::process::id()));
        std::fs::write(&path, b"\x89PNG fake").unwrap();
        let out = content(json!({"text": "The page, 800×600.", "image": path}));
        assert_eq!(out[0]["type"], "image");
        assert_eq!(out[0]["mimeType"], "image/png");
        assert_eq!(out[1]["text"], "The page, 800×600.");
        assert!(!path.exists(), "the file is removed once read");
        // Plain text, and structured answers as JSON.
        assert_eq!(content(json!("Clicked."))[0]["text"], "Clicked.");
        let tabs = content(json!({"tabs": [1]}));
        assert!(tabs[0]["text"].as_str().unwrap().contains("\"tabs\""));
    }

    #[test]
    fn calls_reach_trek_and_come_back() {
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
        #[cfg(unix)]
        let path = std::env::temp_dir().join(format!("trek-mcp-br-{}.sock", std::process::id()));
        #[cfg(windows)]
        let path = trek_ipc::pipe_name(1).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut listener = {
            let _enter = rt.enter();
            trek_ipc::server::Listener::bind(&path).unwrap()
        };
        let server = std::thread::spawn(move || {
            rt.block_on(async {
                let (rd, mut w) = tokio::io::split(listener.accept().await.unwrap());
                let mut r = tokio::io::BufReader::new(rd);
                let mut line = String::new();
                r.read_line(&mut line).await.unwrap();
                w.write_all(trek_ipc::encode(&json!({"ok": true})).as_bytes())
                    .await
                    .unwrap();
                line.clear();
                r.read_line(&mut line).await.unwrap();
                let req: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(req["method"], "browser_click");
                assert_eq!(req["params"]["ref"], "e3");
                w.write_all(
                    trek_ipc::encode(&trek_ipc::reply(
                        &req["id"],
                        Ok(json!({"text": "Clicked button \"Go\"."})),
                    ))
                    .as_bytes(),
                )
                .await
                .unwrap();
            })
        });
        let mut tools = Browser {
            client: Some(trek_ipc::Client {
                socket: path.clone(),
                token: "t".into(),
                session: "s".into(),
            }),
        };
        let out = tools.call("browser_click", &json!({"ref": "e3"})).unwrap();
        assert_eq!(out[0]["text"], "Clicked button \"Go\".");
        server.join().unwrap();
        #[cfg(unix)]
        let _ = std::fs::remove_file(path);
    }
}
