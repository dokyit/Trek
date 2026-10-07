//! A small LSP client for the in-app editor: one language server per (IDE root,
//! language), JSON-RPC over stdio. Hover, completion, go-to-definition and
//! diagnostics get wired into the editor's provider slots; a language with no
//! server on PATH just stays silent — the editor keeps working without it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use anyhow::anyhow;
use gpui_kit::base::input::{CompletionProvider, DefinitionProvider, HoverProvider, Lsp, Point, RopeExt};
use ropey::Rope;
use serde_json::{Value, json};

/// Server binary + args + the LSP `languageId`, per file extension (or grammar
/// name — either works).
pub fn spec(ext: &str) -> Option<(&'static str, &'static [&'static str], &'static str)> {
    match ext {
        "rs" | "rust" => Some(("rust-analyzer", &[], "rust")),
        "swift" => Some(("sourcekit-lsp", &[], "swift")),
        "ts" | "tsx" | "mts" | "cts" | "typescript" | "typescriptreact" => Some(("typescript-language-server", &["--stdio"], "typescript")),
        "js" | "jsx" | "mjs" | "cjs" | "javascript" | "javascriptreact" => Some(("typescript-language-server", &["--stdio"], "javascript")),
        "py" | "python" => Some(("pyright-langserver", &["--stdio"], "python")),
        "go" => Some(("gopls", &[], "go")),
        "c" | "h" => Some(("clangd", &[], "c")),
        "cpp" | "cc" | "cxx" | "hpp" | "c++" => Some(("clangd", &[], "cpp")),
        _ => None,
    }
}

/// `file://` URI for a path — every byte outside the unreserved set is %XX'd
/// (paths with spaces or non-ASCII still parse server-side).
fn uri_of(path: &Path) -> Option<String> {
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~/";
    let mut s = String::from("file://");
    for b in path.to_str()?.bytes() {
        if UNRESERVED.contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    Some(s)
}

/// The file path a `file://` URI names; anything else is None.
fn path_of_uri(uri: &str) -> Option<PathBuf> {
    let s = uri.strip_prefix("file://")?;
    let mut out = Vec::with_capacity(s.len());
    let mut b = s.bytes().peekable();
    while let Some(c) = b.next() {
        if c == b'%' {
            let h = (b.next()? as char).to_digit(16)?;
            let l = (b.next()? as char).to_digit(16)?;
            out.push((h * 16 + l) as u8);
        } else {
            out.push(c);
        }
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

/// Byte offset → LSP position (line + UTF-16 code units within it).
fn position_of(text: &Rope, offset: usize) -> lsp_types::Position {
    let end = offset.min(text.len());
    let point = text.offset_to_point(end);
    let line_start = text.point_to_offset(Point::new(point.row, 0));
    let cu = text.offset_to_offset_utf16(end).saturating_sub(text.offset_to_offset_utf16(line_start));
    lsp_types::Position { line: point.row as u32, character: cu as u32 }
}

/// A language server process. Cheap to share: requests multiplex on one pipe.
pub struct Client {
    _child: Mutex<Child>,
    stdin: Mutex<std::process::ChildStdin>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, async_channel::Sender<Value>>>,
    ready: AtomicBool,
    lang: &'static str,
    /// Every doc's `publishDiagnostics`, broadcast to watchers (they filter by URI).
    diag_senders: Mutex<Vec<async_channel::Sender<(String, Vec<lsp_types::Diagnostic>)>>>,
    /// Messages sent before the `initialized` handshake — flushed once it's sent
    /// (LSP kills the session on early traffic, rust-analyzer literally errors).
    outbox: Mutex<Vec<Value>>,
    version: AtomicU64,
}

impl Client {
    /// Spawn the server for `lang` rooted at `root`, or None when the binary
    /// isn't installed.
    pub fn start(root: PathBuf, lang: &str) -> Option<Arc<Self>> {
        let (binary, args, language_id) = spec(lang)?;
        tracing::info!("lsp: starting {binary} for {lang} at {}", root.display());
        let mut child = Command::new(binary)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| tracing::info!("lsp: {binary} failed to spawn: {e}"))
            .ok()?;
        let stdout = child.stdout.take()?;
        let stdin = child.stdin.take()?;
        let client = Arc::new(Self {
            _child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            ready: AtomicBool::new(false),
            lang: language_id,
            diag_senders: Mutex::new(vec![]),
            outbox: Mutex::new(vec![]),
            version: AtomicU64::new(1),
        });

        // The reader: responses go to the requester, notifications get routed —
        // diagnostics to watchers, everything else dropped.
        let weak: Weak<Client> = Arc::downgrade(&client);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut len = None;
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => return,
                        _ => {}
                    }
                    let t = line.trim();
                    if t.is_empty() {
                        break;
                    }
                    if let Some(v) = t.strip_prefix("Content-Length:") {
                        len = v.trim().parse().ok();
                    }
                }
                let Some(n) = len else { continue };
                let mut buf = vec![0u8; n];
                if reader.read_exact(&mut buf).is_err() {
                    return;
                }
                let Ok(msg) = serde_json::from_slice::<Value>(&buf) else { continue };
                let Some(c) = weak.upgrade() else { return };
                if let Some(id) = msg.get("id").and_then(|i| i.as_u64()) {
                    if let Some(tx) = c.pending.lock().unwrap().remove(&id) {
                        let _ = tx.try_send(msg);
                    }
                } else if msg.get("method") == Some(&json!("textDocument/publishDiagnostics")) {
                    let p = &msg["params"];
                    let uri = p["uri"].as_str().unwrap_or_default().to_string();
                    let diags = serde_json::from_value::<Vec<lsp_types::Diagnostic>>(p["diagnostics"].clone()).unwrap_or_default();
                    c.diag_senders.lock().unwrap().retain(|tx| tx.try_send((uri.clone(), diags.clone())).is_ok());
                }
            }
        });

        // Initialize, then mark ready once the handshake lands.
        let rx = client.request("initialize", initialize_params(&root));
        let weak = Arc::downgrade(&client);
        std::thread::spawn(move || {
            if let Some(c) = weak.upgrade() {
                if let Ok(msg) = rx.recv_blocking() {
                    if msg.get("result").is_some() {
                        c.send(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
                        c.ready.store(true, Ordering::SeqCst);
                        for msg in c.outbox.lock().unwrap().drain(..) {
                            c.send(&msg);
                        }
                    }
                }
            }
        });
        Some(client)
    }

    fn send(&self, msg: &Value) {
        let body = msg.to_string();
        let _ = write!(self.stdin.lock().unwrap(), "Content-Length: {}\r\n\r\n{}", body.len(), body);
        let _ = self.stdin.lock().unwrap().flush();
    }

    fn request(&self, method: &str, params: Value) -> async_channel::Receiver<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = async_channel::bounded(1);
        self.pending.lock().unwrap().insert(id, tx);
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        rx
    }

    /// Notifications wait for the handshake — except `initialized` itself.
    fn notify(&self, method: &str, params: Value) {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        if self.ready.load(Ordering::SeqCst) || method == "initialized" {
            self.send(&msg);
        } else {
            self.outbox.lock().unwrap().push(msg);
        }
    }

    /// Newest diagnostics for every open doc; watchers filter to their URI.
    pub fn watch_diagnostics(&self) -> async_channel::Receiver<(String, Vec<lsp_types::Diagnostic>)> {
        let (tx, rx) = async_channel::unbounded();
        self.diag_senders.lock().unwrap().push(tx);
        rx
    }

    // ---- document sync ----

    pub fn did_open(&self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri, "languageId": self.lang,
                "version": self.version.fetch_add(1, Ordering::SeqCst) as i64,
                "text": text,
            }}),
        );
    }

    /// Full-document sync — legal under `TextDocumentSyncKind::Full`, and the
    /// simple way to never drift.
    pub fn did_change(&self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri, "version": self.version.fetch_add(1, Ordering::SeqCst) as i64 },
                    "contentChanges": [{ "text": text }] }),
        );
    }

    pub fn did_save(&self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": uri }, "text": text }),
        );
    }

    pub fn did_close(&self, uri: &str) {
        self.notify("textDocument/didClose", json!({ "textDocument": { "uri": uri } }));
    }

    fn doc(&self, uri: &str) -> Value {
        json!({ "uri": uri })
    }

    /// The provider bundle one editor installs on its `EditorState`.
    pub fn lsp_for(self: &Arc<Self>, uri: String, workspace: gpui_kit::Entity<crate::workspace::Workspace>) -> Lsp {
        let doc = std::rc::Rc::new(DocProviders { client: self.clone(), uri, workspace });
        let mut lsp = Lsp::default();
        lsp.completion_provider = Some(doc.clone());
        lsp.hover_provider = Some(doc.clone());
        lsp.definition_provider = Some(doc.clone());
        lsp.show_document = Some(std::rc::Rc::new({
            let doc = doc.clone();
            move |params: &lsp_types::ShowDocumentParams, _window, cx| {
                let Some(path) = path_of_uri(params.uri.as_str()) else { return false };
                let line = params.selection.map(|r| r.start.line + 1);
                doc.workspace.update(cx, |ws, cx| ws.open_editor(path, line, cx));
                true
            }
        }));
        lsp
    }
}

fn initialize_params(root: &Path) -> Value {
    json!({
        "processId": std::process::id(),
        "clientInfo": { "name": "Trek", "version": env!("CARGO_PKG_VERSION") },
        "rootUri": uri_of(root),
        "capabilities": {
            "textDocument": {
                "synchronization": { "didSave": true },
                "hover": { "contentFormat": ["markdown", "plaintext"] },
                "completion": { "completionItem": { "documentationFormat": ["markdown", "plaintext"], "snippetSupport": false } },
                "definition": { "linkSupport": true },
                "publishDiagnostics": {},
            },
            "workspace": {},
        },
    })
}

/// One open document's providers — they carry the doc URI into every request.
struct DocProviders {
    client: Arc<Client>,
    uri: String,
    workspace: gpui_kit::Entity<crate::workspace::Workspace>,
}

impl DocProviders {
    /// Await the init handshake, then send the request and unwrap `result`.
    async fn call(client: &Arc<Client>, executor: &gpui_kit::BackgroundExecutor, method: &'static str, params: Value) -> anyhow::Result<Value> {
        // The first request may beat the handshake — give it a moment.
        for _ in 0..100 {
            if client.ready.load(Ordering::SeqCst) {
                break;
            }
            executor.timer(std::time::Duration::from_millis(50)).await;
        }
        let rx = client.request(method, params);
        let msg = rx.recv().await.map_err(|_| anyhow!("server closed"))?;
        if let Some(err) = msg.get("error") {
            return Err(anyhow!("{method}: {}", err.get("message").and_then(|m| m.as_str()).unwrap_or("error")));
        }
        Ok(msg["result"].clone())
    }
}

impl HoverProvider for DocProviders {
    fn hover(&self, text: &Rope, offset: usize, _window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) -> gpui_kit::Task<anyhow::Result<Option<lsp_types::Hover>>> {
        let position = position_of(text, offset);
        let params = json!({ "textDocument": self.client.doc(&self.uri), "position": position });
        let client = self.client.clone();
        cx.spawn(move |cx: &mut gpui_kit::AsyncApp| {
            let executor = cx.background_executor().clone();
            async move {
            let v = DocProviders::call(&client, &executor, "textDocument/hover", params).await?;
            Ok(serde_json::from_value::<Option<lsp_types::Hover>>(v).unwrap_or(None))
            }
        })
    }
}

impl CompletionProvider for DocProviders {
    /// Any typed character can start a completion — the server decides what it has to offer.
    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut gpui_kit::App) -> bool {
        new_text.chars().last().is_some_and(|c| c.is_alphanumeric() || matches!(c, '.' | '>' | ':' | '"' | '/' | '@' | '#'))
    }

    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: lsp_types::CompletionContext,
        _window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::App,
    ) -> gpui_kit::Task<anyhow::Result<lsp_types::CompletionResponse>> {
        let position = position_of(text, offset);
        let params = json!({ "textDocument": self.client.doc(&self.uri), "position": position, "context": trigger });
        let client = self.client.clone();
        cx.spawn(move |cx: &mut gpui_kit::AsyncApp| {
            let executor = cx.background_executor().clone();
            async move {
            let v = DocProviders::call(&client, &executor, "textDocument/completion", params).await?;
            Ok(serde_json::from_value::<Option<lsp_types::CompletionResponse>>(v)
                .ok()
                .flatten()
                .unwrap_or(lsp_types::CompletionResponse::Array(vec![])))
            }
        })
    }
}

impl DefinitionProvider for DocProviders {
    fn definitions(&self, text: &Rope, offset: usize, _window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) -> gpui_kit::Task<anyhow::Result<Vec<lsp_types::LocationLink>>> {
        let position = position_of(text, offset);
        let params = json!({ "textDocument": self.client.doc(&self.uri), "position": position });
        let client = self.client.clone();
        cx.spawn(move |cx: &mut gpui_kit::AsyncApp| {
            let executor = cx.background_executor().clone();
            async move {
            let v = DocProviders::call(&client, &executor, "textDocument/definition", params).await?;
            // Servers answer Location or LocationLink — normalize to links.
            if let Ok(links) = serde_json::from_value::<Option<Vec<lsp_types::LocationLink>>>(v.clone()) {
                return Ok(links.unwrap_or_default());
            }
            let locs: Vec<lsp_types::Location> = serde_json::from_value::<Option<lsp_types::GotoDefinitionResponse>>(v)
                .ok()
                .flatten()
                .map(|r| match r {
                    lsp_types::GotoDefinitionResponse::Scalar(l) => vec![l],
                    lsp_types::GotoDefinitionResponse::Array(v) => v,
                    lsp_types::GotoDefinitionResponse::Link(_) => vec![],
                })
                .unwrap_or_default();
            Ok(locs
                .into_iter()
                .map(|l| lsp_types::LocationLink {
                    origin_selection_range: None,
                    target_uri: l.uri,
                    target_range: l.range,
                    target_selection_range: l.range,
                })
                .collect())
            }
        })
    }
}

/// The URI an editor registers under; None if the path can't become a file URI.
pub fn uri_for(path: &Path) -> Option<String> {
    uri_of(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_analyzer_answers_initialize() {
        let client = Client::start(PathBuf::from("/tmp"), "rs").expect("rust-analyzer spawns");
        // ready flips once the initialize handshake lands
        for _ in 0..600 {
            if client.ready.load(Ordering::SeqCst) { return; }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("initialize never completed");
    }
}
