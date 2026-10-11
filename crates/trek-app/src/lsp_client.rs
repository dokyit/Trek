//! A small LSP client for the in-app editor: one language server per (IDE root,
//! language), JSON-RPC over stdio. Hover, completion, go-to-definition and
//! diagnostics get wired into the editor's provider slots; a language with no
//! server on the login PATH just stays silent — the editor keeps working without it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak, mpsc};
use std::time::{Duration, Instant};

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

/// The longest message a server may send: a bigger `Content-Length` means a broken stream.
const MAX_MESSAGE: usize = 64 << 20;
/// The longest header line read before the stream is given up on.
const MAX_HEADER: u64 = 4096;
/// How long a server gets to answer `initialize` before it's stopped.
const INIT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a stopping server gets to answer `shutdown`, then to exit after `exit`.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
const EXIT_GRACE: Duration = Duration::from_millis(500);
/// Roots whose servers stay running: opening a file under one more stops the oldest root's.
const LIVE_ROOTS: usize = 3;

type Pending = Arc<Mutex<HashMap<u64, async_channel::Sender<Value>>>>;
type DiagnosticsSender = async_channel::Sender<(String, Vec<lsp_types::Diagnostic>)>;

/// A lock that outlives a panic elsewhere (stopping runs from `Drop`).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A language server process. Cheap to share: requests multiplex on one pipe.
pub struct Client {
    /// The server, until it's stopped; its pid is its process group.
    child: Mutex<Option<Child>>,
    /// Frames for the writer thread; None once stopped.
    writer: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    next_id: AtomicU64,
    pending: Pending,
    ready: AtomicBool,
    stopped: AtomicBool,
    lang: &'static str,
    /// Every doc's `publishDiagnostics`, broadcast to watchers (they filter by URI).
    diag_senders: Mutex<Vec<DiagnosticsSender>>,
    /// Messages sent before the `initialized` handshake — flushed once it's sent
    /// (LSP kills the session on early traffic, rust-analyzer literally errors).
    outbox: Mutex<Vec<Value>>,
    version: AtomicU64,
}

impl Client {
    /// Spawn the server for `lang` rooted at `root`, or None when the binary
    /// isn't installed. Found on, and run with, the login shell's PATH.
    pub fn start(root: PathBuf, lang: &str) -> Option<Arc<Self>> {
        let (binary, args, language_id) = spec(lang)?;
        let Some(program) = trek_core::detect::which(binary) else {
            tracing::info!("lsp: {binary} isn't on PATH");
            return None;
        };
        tracing::info!("lsp: starting {binary} for {lang} at {}", root.display());
        let mut command = Command::new(program);
        command.args(args).env("PATH", trek_core::detect::login_path());
        Self::spawn(command, &root, language_id)
    }

    /// Run `command` as a server, in a process group of its own (on Windows, a job object).
    fn spawn(mut command: Command, root: &Path, language_id: &'static str) -> Option<Arc<Self>> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        #[cfg(windows)]
        crate::job::prepare(&mut command);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| tracing::info!("lsp: {:?} failed to spawn: {e}", command.get_program()))
            .ok()?;
        #[cfg(windows)]
        crate::job::adopt(&child);
        trek_core::procs::register(child.id() as i32);
        let (Some(stdout), Some(stdin)) = (child.stdout.take(), child.stdin.take()) else {
            end_child(child, Duration::ZERO);
            return None;
        };
        Some(Self::with_io(stdout, stdin, Some(child), root, language_id))
    }

    /// A client talking over `input`/`output`, sending `initialize` right away.
    fn with_io(input: impl Read + Send + 'static, output: impl Write + Send + 'static, child: Option<Child>, root: &Path, lang: &'static str) -> Arc<Self> {
        // The writer: whole documents go down the pipe here, never on the caller's thread.
        let (tx, frames) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || write_frames(output, frames));
        let client = Arc::new(Self {
            child: Mutex::new(child),
            writer: Mutex::new(Some(tx)),
            next_id: AtomicU64::new(1),
            pending: Arc::default(),
            ready: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            lang,
            diag_senders: Mutex::new(vec![]),
            outbox: Mutex::new(vec![]),
            version: AtomicU64::new(1),
        });

        // The reader: replies go to the requester (also while stopping, after the client is
        // dropped), the server's own requests get answered, diagnostics go to watchers.
        let weak: Weak<Client> = Arc::downgrade(&client);
        let pending = client.pending.clone();
        std::thread::spawn(move || {
            let mut input = BufReader::new(input);
            loop {
                let msg = match read_frame(&mut input) {
                    Frame::Message(msg) => msg,
                    Frame::Skip => continue,
                    Frame::End => break,
                };
                match kind(&msg) {
                    Kind::Reply(id) => {
                        if let Some(tx) = id.and_then(|id| lock(&pending).remove(&id)) {
                            let _ = tx.try_send(msg);
                        }
                    }
                    Kind::Request | Kind::Notification => {
                        if let Some(c) = weak.upgrade() {
                            c.handle(&msg);
                        }
                    }
                    Kind::Invalid => {}
                }
            }
            // The server exited or its stream broke: nothing more is coming.
            if let Some(c) = weak.upgrade() {
                c.stop();
            }
            lock(&pending).clear();
        });

        // Initialize, then mark ready once the handshake lands. Only a weak ref waits, so a
        // client dropped meanwhile still stops; a server that never answers is stopped too.
        let rx = client.request("initialize", initialize_params(root));
        let weak = Arc::downgrade(&client);
        std::thread::spawn(move || {
            let reply = recv_timeout(&rx, INIT_TIMEOUT);
            let Some(c) = weak.upgrade() else { return };
            match reply {
                Some(msg) if msg.get("result").is_some() => c.initialized(),
                _ => {
                    tracing::info!("lsp: {} server didn't initialize", c.lang);
                    c.stop();
                }
            }
        });
        client
    }

    /// Send `initialized`, then everything held back for it.
    fn initialized(&self) {
        let mut outbox = lock(&self.outbox);
        self.send(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
        self.ready.store(true, Ordering::SeqCst);
        for msg in outbox.drain(..) {
            self.send(&msg);
        }
    }

    /// A request or notification from the server.
    fn handle(&self, msg: &Value) {
        match kind(msg) {
            Kind::Request => self.send(&answer(msg)),
            Kind::Notification if msg["method"] == "textDocument/publishDiagnostics" => {
                let p = &msg["params"];
                let uri = p["uri"].as_str().unwrap_or_default().to_string();
                let diags = serde_json::from_value::<Vec<lsp_types::Diagnostic>>(p["diagnostics"].clone()).unwrap_or_default();
                lock(&self.diag_senders).retain(|tx| tx.try_send((uri.clone(), diags.clone())).is_ok());
            }
            _ => {}
        }
    }

    /// Queue `msg` for the writer thread; never blocks. Dropped once stopped.
    fn send(&self, msg: &Value) {
        if let Some(w) = lock(&self.writer).as_ref() {
            let _ = w.send(frame(msg));
        }
    }

    fn request(&self, method: &str, params: Value) -> async_channel::Receiver<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = async_channel::bounded(1);
        lock(&self.pending).insert(id, tx);
        // Stopped: no reply will come, so the receiver closes now.
        if self.stopped.load(Ordering::SeqCst) {
            lock(&self.pending).remove(&id);
            return rx;
        }
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        rx
    }

    /// Notifications wait for the handshake.
    fn notify(&self, method: &str, params: Value) {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let mut outbox = lock(&self.outbox);
        if self.ready.load(Ordering::SeqCst) {
            self.send(&msg);
        } else if !self.stopped.load(Ordering::SeqCst) {
            outbox.push(msg);
        }
    }

    /// Whether the server was stopped (or exited): requests to it fail.
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Stop the server: `shutdown`, `exit`, then its process group is ended and reaped, all on
    /// a thread of its own. Requests after this fail. Safe to call again.
    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        lock(&self.outbox).clear();
        let writer = lock(&self.writer).take();
        let child = lock(&self.child).take();
        let shutdown = self.ready.load(Ordering::SeqCst).then(|| {
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = async_channel::bounded(1);
            lock(&self.pending).insert(id, tx);
            (id, rx)
        });
        std::thread::spawn(move || {
            if let Some(w) = writer {
                if let Some((id, rx)) = shutdown {
                    let _ = w.send(frame(&json!({ "jsonrpc": "2.0", "id": id, "method": "shutdown" })));
                    recv_timeout(&rx, SHUTDOWN_GRACE);
                }
                let _ = w.send(frame(&json!({ "jsonrpc": "2.0", "method": "exit" })));
                // Dropping the last sender ends the writer thread, which closes stdin.
            }
            if let Some(child) = child {
                end_child(child, EXIT_GRACE);
            }
        });
    }

    /// Newest diagnostics for every open doc; watchers filter to their URI.
    pub fn watch_diagnostics(&self) -> async_channel::Receiver<(String, Vec<lsp_types::Diagnostic>)> {
        let (tx, rx) = async_channel::unbounded();
        lock(&self.diag_senders).push(tx);
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
        let doc = std::rc::Rc::new(DocProviders { client: self.clone(), uri, workspace: workspace.downgrade() });
        let mut lsp = Lsp::default();
        lsp.completion_provider = Some(doc.clone());
        lsp.hover_provider = Some(doc.clone());
        lsp.definition_provider = Some(doc.clone());
        lsp.show_document = Some(std::rc::Rc::new({
            let doc = doc.clone();
            move |params: &lsp_types::ShowDocumentParams, _window, cx| {
                let Some(path) = path_of_uri(params.uri.as_str()) else { return false };
                let line = params.selection.map(|r| r.start.line + 1);
                doc.workspace.update(cx, |ws, cx| ws.open_editor(path, line, cx)).is_ok()
            }
        }));
        lsp
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stop();
    }
}

/// One framed message: `Content-Length` header, blank line, JSON body.
fn frame(msg: &Value) -> Vec<u8> {
    let body = msg.to_string();
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

/// The writer thread: frames go out in order until the client stops or the pipe breaks.
fn write_frames(mut output: impl Write, frames: mpsc::Receiver<Vec<u8>>) {
    for f in frames {
        if output.write_all(&f).and_then(|_| output.flush()).is_err() {
            return;
        }
    }
}

/// What the reader got off the stream.
#[derive(Debug, PartialEq)]
enum Frame {
    Message(Value),
    /// A frame without a length or a body that isn't JSON: skipped.
    Skip,
    /// End of stream, or a stream that can't be trusted (an absurd or garbled length).
    End,
}

fn read_frame(input: &mut impl BufRead) -> Frame {
    let mut len = None;
    let mut line = String::new();
    loop {
        line.clear();
        match input.by_ref().take(MAX_HEADER).read_line(&mut line) {
            Ok(0) | Err(_) => return Frame::End,
            _ if !line.ends_with('\n') => return Frame::End,
            _ => {}
        }
        let t = line.trim();
        if t.is_empty() {
            break;
        }
        if let Some(v) = t.strip_prefix("Content-Length:") {
            match v.trim().parse::<usize>() {
                Ok(n) if n <= MAX_MESSAGE => len = Some(n),
                _ => return Frame::End,
            }
        }
    }
    let Some(n) = len else { return Frame::Skip };
    let mut buf = vec![0u8; n];
    if input.read_exact(&mut buf).is_err() {
        return Frame::End;
    }
    serde_json::from_slice(&buf).map_or(Frame::Skip, Frame::Message)
}

/// A JSON-RPC message by shape.
#[derive(Debug, PartialEq)]
enum Kind {
    /// An answer to one of Trek's requests (ids Trek sends are numbers).
    Reply(Option<u64>),
    /// The server asking Trek something: it wants an answer.
    Request,
    Notification,
    Invalid,
}

fn kind(msg: &Value) -> Kind {
    let id = msg.get("id").filter(|id| !id.is_null());
    match (msg.get("method").is_some_and(Value::is_string), id) {
        (true, Some(_)) => Kind::Request,
        (true, None) => Kind::Notification,
        (false, Some(id)) => Kind::Reply(id.as_u64()),
        (false, None) => Kind::Invalid,
    }
}

/// The answer to a request from the server: the few with an obvious answer get it, the rest
/// MethodNotFound.
fn answer(request: &Value) -> Value {
    let method = request["method"].as_str().unwrap_or_default();
    let result = match method {
        // Nothing configured: one null per item asked for, so the server keeps its defaults.
        "workspace/configuration" => Some(Value::Array(vec![Value::Null; request["params"]["items"].as_array().map_or(0, Vec::len)])),
        "window/workDoneProgress/create" | "client/registerCapability" | "client/unregisterCapability" => Some(Value::Null),
        _ => None,
    };
    match result {
        Some(result) => json!({ "jsonrpc": "2.0", "id": request["id"], "result": result }),
        None => json!({ "jsonrpc": "2.0", "id": request["id"], "error": { "code": -32601, "message": format!("{method} isn't supported") } }),
    }
}

/// Wait up to `limit` for a reply, without holding anything but the receiver.
fn recv_timeout(rx: &async_channel::Receiver<Value>, limit: Duration) -> Option<Value> {
    let deadline = Instant::now() + limit;
    loop {
        match rx.try_recv() {
            Ok(msg) => return Some(msg),
            Err(async_channel::TryRecvError::Closed) => return None,
            Err(async_channel::TryRecvError::Empty) if Instant::now() >= deadline => return None,
            Err(async_channel::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// Give a server up to `grace` to exit, then KILL its group (and whatever it started) and reap it.
fn end_child(mut child: Child, grace: Duration) {
    let group = child.id() as i32;
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
        std::thread::sleep(Duration::from_millis(20));
    }
    #[cfg(unix)]
    if group > 1 {
        // SAFETY: a negative pid asks kill(2) to signal that process group.
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }
    #[cfg(windows)]
    crate::job::terminate(child.id());
    let _ = child.kill();
    let _ = child.wait();
    trek_core::procs::unregister(group);
}

/// The live servers, one per (root, language), for the few most recently used roots.
pub struct Servers {
    by_key: HashMap<(PathBuf, &'static str), Arc<Client>>,
    roots: RecentRoots,
}

impl Default for Servers {
    fn default() -> Self {
        Self { by_key: HashMap::new(), roots: RecentRoots::new(LIVE_ROOTS) }
    }
}

impl Servers {
    /// The server for `lang` at `root`, started if there's none (or it stopped). Servers of
    /// the root that falls out of the most recent few are stopped.
    pub fn get_or_start(&mut self, root: PathBuf, lang: &str) -> Option<Arc<Client>> {
        let (_, _, language_id) = spec(lang)?;
        for old in self.roots.touch(&root) {
            self.by_key.retain(|(r, _), c| {
                if *r == old {
                    c.stop();
                }
                *r != old
            });
        }
        let key = (root, language_id);
        if let Some(c) = self.by_key.get(&key).filter(|c| !c.stopped()) {
            return Some(c.clone());
        }
        let c = Client::start(key.0.clone(), lang)?;
        self.by_key.insert(key, c.clone());
        Some(c)
    }
}

/// Roots by when their servers were last asked for, oldest first, at most `cap`.
#[derive(Debug)]
struct RecentRoots {
    roots: Vec<PathBuf>,
    cap: usize,
}

impl RecentRoots {
    fn new(cap: usize) -> Self {
        Self { roots: vec![], cap }
    }

    /// Mark `root` as just used; returns the roots that fell out.
    fn touch(&mut self, root: &Path) -> Vec<PathBuf> {
        self.roots.retain(|r| r != root);
        self.roots.push(root.to_path_buf());
        let over = self.roots.len().saturating_sub(self.cap);
        self.roots.drain(..over).collect()
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
    /// Weak: the providers live in the editor's state, which the workspace outlives.
    workspace: gpui_kit::WeakEntity<crate::workspace::Workspace>,
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
    use std::io::Cursor;
    #[cfg(unix)]
    use std::os::unix::net::UnixStream as Socket;

    /// Windows has no socketpair: a connected pair over loopback does the same.
    #[cfg(windows)]
    use std::net::TcpStream as Socket;

    #[cfg(unix)]
    fn socket_pair() -> (Socket, Socket) {
        Socket::pair().unwrap()
    }

    #[cfg(windows)]
    fn socket_pair() -> (Socket, Socket) {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let ours = Socket::connect(listener.local_addr().unwrap()).unwrap();
        let (theirs, _) = listener.accept().unwrap();
        for socket in [&ours, &theirs] {
            socket.set_nodelay(true).unwrap();
        }
        (ours, theirs)
    }

    /// A client wired to an in-process "server": what it writes, and a handle to write back.
    fn fake() -> (Arc<Client>, Socket, BufReader<Socket>) {
        let (ours, theirs) = socket_pair();
        theirs.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let client = Client::with_io(ours.try_clone().unwrap(), ours, None, Path::new("/tmp"), "rust");
        let from_client = BufReader::new(theirs.try_clone().unwrap());
        (client, theirs, from_client)
    }

    fn next(from_client: &mut BufReader<Socket>) -> Value {
        match read_frame(from_client) {
            Frame::Message(msg) => msg,
            other => panic!("expected a message, got {other:?}"),
        }
    }

    fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ok() {
            assert!(Instant::now() < deadline, "timed out waiting: {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn lsp_frames_are_read_and_absurd_lengths_end_the_stream() {
        let mut ok = Cursor::new(b"Content-Length: 2\r\n\r\n{}Content-Length: 3\r\n\r\nnopX-Other: 1\r\n\r\nContent-Length: 2\r\n\r\n[]".to_vec());
        assert_eq!(read_frame(&mut ok), Frame::Message(json!({})));
        assert_eq!(read_frame(&mut ok), Frame::Skip, "not JSON");
        assert_eq!(read_frame(&mut ok), Frame::Skip, "no length");
        assert_eq!(read_frame(&mut ok), Frame::Message(json!([])));
        assert_eq!(read_frame(&mut ok), Frame::End);

        let huge = format!("Content-Length: {}\r\n\r\n{{}}", MAX_MESSAGE + 1);
        assert_eq!(read_frame(&mut Cursor::new(huge.into_bytes())), Frame::End);
        assert_eq!(read_frame(&mut Cursor::new(b"Content-Length: 99999999999999999999999\r\n\r\n".to_vec())), Frame::End);
        assert_eq!(read_frame(&mut Cursor::new(b"Content-Length: lots\r\n\r\n{}".to_vec())), Frame::End);
        assert_eq!(read_frame(&mut Cursor::new(vec![b'a'; 10_000])), Frame::End, "a header line without end");
        assert_eq!(read_frame(&mut Cursor::new(b"Content-Length: 10\r\n\r\n{}".to_vec())), Frame::End, "short body");
    }

    #[test]
    fn lsp_messages_are_told_apart_by_shape() {
        assert_eq!(kind(&json!({ "id": 3, "result": null })), Kind::Reply(Some(3)));
        assert_eq!(kind(&json!({ "id": "x", "error": {} })), Kind::Reply(None));
        assert_eq!(kind(&json!({ "id": 3, "method": "workspace/configuration" })), Kind::Request);
        assert_eq!(kind(&json!({ "method": "window/logMessage" })), Kind::Notification);
        assert_eq!(kind(&json!({ "id": null, "method": "x" })), Kind::Notification);
        assert_eq!(kind(&json!({})), Kind::Invalid);
    }

    #[test]
    fn lsp_server_requests_are_answered_not_taken_for_replies() {
        let (client, mut server, mut from_client) = fake();
        let init = next(&mut from_client);
        assert_eq!(init["method"], "initialize");
        let id = init["id"].clone();

        // The server asks something with the same id as Trek's pending `initialize`.
        server.write_all(&frame(&json!({ "jsonrpc": "2.0", "id": id, "method": "workspace/diagnostic/refresh" }))).unwrap();
        let reply = next(&mut from_client);
        assert_eq!(reply["id"], id);
        assert_eq!(reply["error"]["code"], -32601);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!client.ready.load(Ordering::SeqCst), "not mistaken for the initialize reply");

        server.write_all(&frame(&json!({ "jsonrpc": "2.0", "id": "cfg", "method": "workspace/configuration", "params": { "items": [{}, {}] } }))).unwrap();
        let reply = next(&mut from_client);
        assert_eq!((reply["id"].clone(), reply["result"].clone()), (json!("cfg"), json!([null, null])));
        server.write_all(&frame(&json!({ "jsonrpc": "2.0", "id": 7, "method": "window/workDoneProgress/create", "params": {} }))).unwrap();
        let reply = next(&mut from_client);
        assert_eq!(reply.get("result"), Some(&Value::Null));
        assert!(reply.get("error").is_none());

        // The real reply completes the handshake.
        server.write_all(&frame(&json!({ "jsonrpc": "2.0", "id": id, "result": { "capabilities": {} } }))).unwrap();
        assert_eq!(next(&mut from_client)["method"], "initialized");
        wait_until("ready", || client.ready.load(Ordering::SeqCst));
    }

    #[test]
    fn lsp_notifications_wait_for_the_handshake() {
        let (client, mut server, mut from_client) = fake();
        let id = next(&mut from_client)["id"].clone();
        client.did_open("file:///tmp/a.rs", "fn main() {}");
        server.write_all(&frame(&json!({ "jsonrpc": "2.0", "id": id, "result": {} }))).unwrap();
        assert_eq!(next(&mut from_client)["method"], "initialized");
        assert_eq!(next(&mut from_client)["method"], "textDocument/didOpen");
    }

    #[test]
    fn lsp_send_does_not_wait_for_the_server_to_read() {
        // Nobody reads: a write on the caller's thread would block once the socket buffer fills.
        let (client, _server, _from_client) = fake();
        let doc = "x".repeat(1 << 20);
        let started = Instant::now();
        for _ in 0..16 {
            client.did_change("file:///tmp/a.rs", &doc);
            client.send(&json!({ "text": doc }));
        }
        assert!(started.elapsed() < Duration::from_secs(2), "send blocked for {:?}", started.elapsed());
    }

    #[test]
    fn lsp_requests_fail_once_stopped() {
        let (client, _server, _from_client) = fake();
        client.stop();
        assert!(client.stopped());
        assert!(client.request("textDocument/hover", json!({})).recv_blocking().is_err());
    }

    #[test]
    fn lsp_servers_are_kept_for_the_most_recent_roots() {
        let (a, b, c) = (Path::new("/a"), Path::new("/b"), Path::new("/c"));
        let mut roots = RecentRoots::new(2);
        assert!(roots.touch(a).is_empty());
        assert!(roots.touch(b).is_empty());
        assert!(roots.touch(a).is_empty(), "a again: now the most recent");
        assert_eq!(roots.touch(c), vec![b.to_path_buf()]);
        assert!(roots.touch(a).is_empty());
        assert_eq!(roots.touch(b), vec![c.to_path_buf()]);
        assert_eq!(roots.roots, vec![a.to_path_buf(), b.to_path_buf()]);
    }

    /// Whether `pid` is gone (exited and reaped).
    #[cfg(unix)]
    fn gone(pid: i32) -> bool {
        // SAFETY: signal 0 only checks.
        unsafe { libc::kill(pid, 0) != 0 }
    }

    #[cfg(windows)]
    fn gone(pid: i32) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: plain calls; the handle is closed before returning.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
            if handle.is_null() {
                return true;
            }
            let mut code = 0;
            let running = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
            CloseHandle(handle);
            !running
        }
    }

    #[test]
    fn lsp_server_that_never_answers_is_ended_when_dropped() {
        let mut command = Command::new(trek_test_fixtures::bin("fixture"));
        command.args(["sleep", "30"]);
        let client = Client::spawn(command, Path::new("/tmp"), "rust").unwrap();
        let pid = lock(&client.child).as_ref().unwrap().id() as i32;
        assert!(trek_core::procs::live().contains(&pid));
        drop(client);
        wait_until("the server's group ended and reaped", || gone(pid) && !trek_core::procs::live().contains(&pid));
    }

    #[test]
    fn rust_analyzer_answers_initialize_and_stops() {
        // rustup puts a rust-analyzer on the PATH even where the component isn't installed (CI's
        // toolchain): it starts, says so and exits, and would never answer.
        let works = Command::new("rust-analyzer").arg("--version").output().is_ok_and(|o| o.status.success());
        let Some(client) = Client::spawn(Command::new("rust-analyzer"), Path::new("/tmp"), "rust").filter(|_| works) else {
            eprintln!("rust-analyzer isn't installed; skipped");
            return;
        };
        let pid = lock(&client.child).as_ref().unwrap().id() as i32;
        // ready flips once the initialize handshake lands
        let deadline = Instant::now() + Duration::from_secs(30);
        while !client.ready.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "initialize never completed");
            std::thread::sleep(Duration::from_millis(50));
        }
        client.stop();
        wait_until("rust-analyzer stopped", || gone(pid) && !trek_core::procs::live().contains(&pid));
    }
}
