//! LSP client — talks to `tinymist` for Typst language intelligence.
//!
//! ## Architecture
//!
//! A background thread owns the `tinymist` subprocess and communicates via
//! JSON-RPC 2.0 framed over its stdin/stdout.  `LspHandle` is a cheap
//! clone-able reference the UI thread uses to send requests.  Results come
//! back through a `futures::channel::mpsc` channel that a `cx.spawn` task
//! in `MainWindow` monitors; it then calls into GPUI on the UI thread.
//!
//! ## Graceful degradation
//!
//! If `tinymist` is not installed (or fails to start), `spawn_lsp` returns
//! `None` and all LSP features are silently disabled — no error shown.
//!
//! ## LSP subset implemented
//!
//! | Feature                          | Key      |
//! |----------------------------------|----------|
//! | `textDocument/didOpen`           | on open  |
//! | `textDocument/didChange`         | on edit  |
//! | `textDocument/publishDiagnostics`| gutter   |
//! | `textDocument/hover`             | `K`      |
//! | `textDocument/definition`        | `gd`     |

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

// ── Public types ───────────────────────────────────────────────────────────────

/// A 0-based (line, character) position.
///
/// The LSP protocol uses UTF-16 character offsets; ockr approximates with
/// byte offsets for the ASCII-heavy Typst source.  In practice this is
/// correct for all-ASCII content and may be slightly off for wide Unicode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LspPosition {
    pub line: usize,
    pub character: usize,
}

/// A half-open range `[start, end)` in source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

/// LSP `DiagnosticSeverity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// A diagnostic produced by the language server.
#[derive(Debug, Clone)]
pub struct LspDiagnostic {
    pub severity: LspSeverity,
    /// Human-readable description shown in the status bar on `[d`/`]d` jump.
    #[allow(dead_code)] // surfaced in future status-bar tooltip feature
    pub message: String,
    pub range: LspRange,
}

/// Payload of a successful hover request.
#[derive(Debug, Clone)]
pub struct HoverResult {
    /// Markdown or plain-text content (raw, not rendered).
    pub content: String,
}

/// Payload of a successful go-to-definition request.
#[derive(Debug, Clone)]
pub struct DefinitionResult {
    /// Absolute path of the file containing the definition.
    pub path: PathBuf,
    /// 0-based line.
    pub line: usize,
    /// 0-based character offset.
    pub col: usize,
}

/// One completion candidate.
#[derive(Debug, Clone)]
pub struct CompletionItem {
    /// Text shown in the popup.
    pub label: String,
    /// Text inserted on accept (falls back to `label` when the server omits it).
    pub insert_text: String,
    /// Optional type/detail shown dimmed beside the label.
    pub detail: Option<String>,
}

/// A message produced by the LSP thread and delivered to the UI thread.
pub enum LspMessage {
    /// Diagnostics for the given URI (may be empty = all clear).
    Diagnostics {
        uri: String,
        diags: Vec<LspDiagnostic>,
    },
    /// Result of a `textDocument/hover` request.
    HoverResult {
        request_id: i64,
        result: Option<HoverResult>,
    },
    /// Result of a `textDocument/definition` request.
    DefinitionResult {
        request_id: i64,
        result: Option<DefinitionResult>,
    },
    /// Result of a `textDocument/completion` request.
    CompletionResult {
        request_id: i64,
        items: Vec<CompletionItem>,
    },
    /// The language server was unavailable or crashed.
    Unavailable,
}

// ── Internal request enum ──────────────────────────────────────────────────────

enum LspRequest {
    DidOpen { uri: String, text: String },
    DidChange { uri: String, version: i32, text: String },
    Hover { uri: String, line: usize, col: usize, id: i64 },
    Definition { uri: String, line: usize, col: usize, id: i64 },
    Completion { uri: String, line: usize, col: usize, id: i64 },
    #[allow(dead_code)] // sent when LspHandle is explicitly shut down
    Shutdown,
}

// ── Handle ─────────────────────────────────────────────────────────────────────

/// Cheap clone-able reference to the LSP background thread.
#[derive(Clone)]
pub struct LspHandle {
    tx: std::sync::mpsc::Sender<LspRequest>,
    next_id: Arc<AtomicI64>,
}

impl LspHandle {
    /// Notify the server that `uri` was opened with the given full text.
    pub fn notify_open(&self, uri: String, text: String) {
        let _ = self.tx.send(LspRequest::DidOpen { uri, text });
    }

    /// Notify the server that `uri` changed (full content sync).
    pub fn notify_change(&self, uri: String, version: i32, text: String) {
        let _ = self.tx.send(LspRequest::DidChange { uri, version, text });
    }

    /// Request hover info at `(line, col)`.  Returns the request ID so the
    /// UI can match the async response.
    pub fn request_hover(&self, uri: String, line: usize, col: usize) -> i64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(LspRequest::Hover { uri, line, col, id });
        id
    }

    /// Request go-to-definition at `(line, col)`.  Returns the request ID.
    pub fn request_definition(&self, uri: String, line: usize, col: usize) -> i64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(LspRequest::Definition { uri, line, col, id });
        id
    }

    /// Request completions at `(line, col)`.  Returns the request ID.
    pub fn request_completion(&self, uri: String, line: usize, col: usize) -> i64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(LspRequest::Completion { uri, line, col, id });
        id
    }
}

// ── Spawn ──────────────────────────────────────────────────────────────────────

/// Spawn the LSP background thread and connect it to `tinymist`.
///
/// Returns `None` if `tinymist` is not found in `PATH`.
/// `on_message` is called from the LSP background thread; the caller is
/// expected to forward messages through a channel to the GPUI UI thread.
pub fn spawn_lsp(
    workspace_root: Option<PathBuf>,
    on_message: impl Fn(LspMessage) + Send + 'static,
) -> Option<LspHandle> {
    let child = Command::new("tinymist")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let (tx, rx) = std::sync::mpsc::channel::<LspRequest>();
    let next_id = Arc::new(AtomicI64::new(100));

    let handle = LspHandle {
        tx,
        next_id: Arc::clone(&next_id),
    };

    std::thread::Builder::new()
        .name("ockr-lsp".into())
        .spawn(move || {
            lsp_thread(child, rx, workspace_root, on_message);
        })
        .expect("failed to spawn LSP thread");

    Some(handle)
}

// ── Background thread ──────────────────────────────────────────────────────────

fn lsp_thread(
    mut child: Child,
    rx: std::sync::mpsc::Receiver<LspRequest>,
    workspace_root: Option<PathBuf>,
    on_message: impl Fn(LspMessage) + Send + 'static,
) {
    let stdin = match child.stdin.take() {
        Some(s) => Arc::new(Mutex::new(s)),
        None => {
            on_message(LspMessage::Unavailable);
            return;
        }
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            on_message(LspMessage::Unavailable);
            return;
        }
    };

    // ── Send initialize ──────────────────────────────────────────────────────
    let root_uri = workspace_root
        .as_deref()
        .map(path_to_uri)
        .unwrap_or_else(|| "file:///".to_string());

    let init = json!({
        "processId": std::process::id(),
        "clientInfo": { "name": "ockr", "version": "0.1.0" },
        "rootUri": root_uri,
        "capabilities": {
            "textDocument": {
                "synchronization": {
                    "dynamicRegistration": false,
                    "willSave": false,
                    "willSaveWaitUntil": false,
                    "didSave": false
                },
                "publishDiagnostics": {
                    "relatedInformation": false,
                    "versionSupport": false
                },
                "hover": {
                    "contentFormat": ["markdown", "plaintext"]
                },
                "definition": {
                    "linkSupport": false
                },
                "completion": {
                    "completionItem": { "snippetSupport": false }
                }
            }
        },
        "initializationOptions": {}
    });

    if send_request(&stdin, 1, "initialize", init).is_err() {
        on_message(LspMessage::Unavailable);
        return;
    }

    // ── Reader thread ────────────────────────────────────────────────────────
    let (read_tx, read_rx) = std::sync::mpsc::channel::<Value>();
    {
        let reader = BufReader::new(stdout);
        std::thread::Builder::new()
            .name("ockr-lsp-reader".into())
            .spawn(move || lsp_reader(reader, read_tx))
            .expect("failed to spawn LSP reader");
    }

    // ── Wait for initialize response ─────────────────────────────────────────
    loop {
        match read_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(msg) if msg.get("id").and_then(|v| v.as_i64()) == Some(1) => break,
            Ok(_) => continue,
            Err(_) => {
                on_message(LspMessage::Unavailable);
                return;
            }
        }
    }

    // Send `initialized` notification.
    let _ = send_notification(&stdin, "initialized", json!({}));

    // ── Main event loop ──────────────────────────────────────────────────────
    // Strategy: block on incoming LSP messages for up to 10 ms, then drain
    // any outgoing UI requests.  This keeps CPU near zero while idle and
    // achieves ~10 ms latency for hover/definition responses.
    let mut pending: HashMap<i64, &'static str> = HashMap::new();

    loop {
        // ── Incoming messages from tinymist ──────────────────────────────────
        match read_rx.recv_timeout(Duration::from_millis(10)) {
            Ok(msg) => {
                dispatch_incoming(&msg, &mut pending, &on_message);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {} // normal idle
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // tinymist exited.
                on_message(LspMessage::Unavailable);
                return;
            }
        }

        // ── Outgoing requests from the UI ────────────────────────────────────
        loop {
            match rx.try_recv() {
                Ok(req) => {
                    if !handle_outgoing(req, &stdin, &mut pending, &mut child) {
                        return; // Shutdown requested
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // LspHandle dropped — graceful shutdown.
                    let _ = send_request(&stdin, 999, "shutdown", json!(null));
                    let _ = send_notification(&stdin, "exit", json!({}));
                    let _ = child.wait();
                    return;
                }
            }
        }
    }
}

/// Returns `false` if a `Shutdown` was processed (caller should exit the loop).
fn handle_outgoing(
    req: LspRequest,
    stdin: &Mutex<ChildStdin>,
    pending: &mut HashMap<i64, &'static str>,
    child: &mut Child,
) -> bool {
    match req {
        LspRequest::DidOpen { uri, text } => {
            let params = json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "typst",
                    "version": 1,
                    "text": text
                }
            });
            let _ = send_notification(stdin, "textDocument/didOpen", params);
        }
        LspRequest::DidChange { uri, version, text } => {
            let params = json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }]
            });
            let _ = send_notification(stdin, "textDocument/didChange", params);
        }
        LspRequest::Hover { uri, line, col, id } => {
            pending.insert(id, "hover");
            let params = json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": col }
            });
            let _ = send_request(stdin, id, "textDocument/hover", params);
        }
        LspRequest::Definition { uri, line, col, id } => {
            pending.insert(id, "definition");
            let params = json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": col }
            });
            let _ = send_request(stdin, id, "textDocument/definition", params);
        }
        LspRequest::Completion { uri, line, col, id } => {
            pending.insert(id, "completion");
            let params = json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": col }
            });
            let _ = send_request(stdin, id, "textDocument/completion", params);
        }
        LspRequest::Shutdown => {
            let _ = send_request(stdin, 999, "shutdown", json!(null));
            let _ = send_notification(stdin, "exit", json!({}));
            let _ = child.wait();
            return false;
        }
    }
    true
}

fn dispatch_incoming(
    msg: &Value,
    pending: &mut HashMap<i64, &'static str>,
    on_message: &impl Fn(LspMessage),
) {
    // Notification: has "method", no "id".
    if let Some(method) = msg.get("method").and_then(|v| v.as_str()) {
        if method == "textDocument/publishDiagnostics" {
            if let Some(params) = msg.get("params") {
                let uri = params["uri"].as_str().unwrap_or("").to_string();
                let diags = parse_diagnostics(&params["diagnostics"]);
                on_message(LspMessage::Diagnostics { uri, diags });
            }
        }
        // Ignore all other server-initiated requests/notifications.
        return;
    }

    // Response: has "id".
    let Some(id) = msg.get("id").and_then(|v| v.as_i64()) else {
        return;
    };

    match pending.remove(&id) {
        Some("hover") => {
            let result = msg
                .get("result")
                .filter(|r| !r.is_null())
                .map(|r| {
                    let content = extract_hover_content(&r["contents"]);
                    HoverResult { content }
                })
                .filter(|h| !h.content.is_empty());
            on_message(LspMessage::HoverResult { request_id: id, result });
        }
        Some("definition") => {
            let result = msg
                .get("result")
                .filter(|r| !r.is_null())
                .and_then(|r| parse_definition_result(r));
            on_message(LspMessage::DefinitionResult { request_id: id, result });
        }
        Some("completion") => {
            let items = msg
                .get("result")
                .filter(|r| !r.is_null())
                .map(parse_completion_items)
                .unwrap_or_default();
            on_message(LspMessage::CompletionResult { request_id: id, items });
        }
        _ => {} // initialize / shutdown / unknown
    }
}

// ── Stdout reader thread ───────────────────────────────────────────────────────

fn lsp_reader(mut reader: BufReader<ChildStdout>, tx: std::sync::mpsc::Sender<Value>) {
    loop {
        // Read all headers until we hit an empty line.
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => return, // EOF
                Ok(_) => {}
                Err(_) => return,
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break; // end of headers
            }
            if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                content_length = rest.trim().parse().ok();
            }
        }

        // Cap the body size so a garbled Content-Length can't trigger a
        // multi-GB allocation (tinymist is trusted, but a corrupt frame isn't).
        const MAX_BODY: usize = 64 * 1024 * 1024;
        let len = match content_length {
            Some(l) if l > 0 && l <= MAX_BODY => l,
            _ => continue,
        };

        // Read exactly `len` bytes of JSON body.
        let mut body = vec![0u8; len];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let Ok(text) = std::str::from_utf8(&body) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        if tx.send(json).is_err() {
            return; // main thread gone
        }
    }
}

// ── Wire helpers ───────────────────────────────────────────────────────────────

fn send_request(
    stdin: &Mutex<ChildStdin>,
    id: i64,
    method: &str,
    params: Value,
) -> std::io::Result<()> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string();
    write_message(stdin, &body)
}

fn send_notification(
    stdin: &Mutex<ChildStdin>,
    method: &str,
    params: Value,
) -> std::io::Result<()> {
    let body = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    })
    .to_string();
    write_message(stdin, &body)
}

fn write_message(stdin: &Mutex<ChildStdin>, body: &str) -> std::io::Result<()> {
    let mut lock = stdin.lock().unwrap();
    write!(lock, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    lock.flush()
}

// ── Parsing helpers ────────────────────────────────────────────────────────────

fn parse_diagnostics(arr: &Value) -> Vec<LspDiagnostic> {
    let Some(arr) = arr.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|d| {
            let message = d["message"].as_str()?.to_string();
            let severity = match d["severity"].as_i64().unwrap_or(1) {
                1 => LspSeverity::Error,
                2 => LspSeverity::Warning,
                3 => LspSeverity::Information,
                _ => LspSeverity::Hint,
            };
            let range = parse_range(&d["range"])?;
            Some(LspDiagnostic { severity, message, range })
        })
        .collect()
}

fn parse_range(r: &Value) -> Option<LspRange> {
    Some(LspRange {
        start: parse_position(&r["start"])?,
        end: parse_position(&r["end"])?,
    })
}

fn parse_position(p: &Value) -> Option<LspPosition> {
    Some(LspPosition {
        line: p["line"].as_u64()? as usize,
        character: p["character"].as_u64()? as usize,
    })
}

fn extract_hover_content(contents: &Value) -> String {
    // MarkupContent: { kind: "markdown"|"plaintext", value: "..." }
    if let Some(value) = contents.get("value").and_then(|v| v.as_str()) {
        return value.trim().to_string();
    }
    // Plain string
    if let Some(s) = contents.as_str() {
        return s.trim().to_string();
    }
    // Array of MarkedString | string
    if let Some(arr) = contents.as_array() {
        let parts: Vec<String> = arr
            .iter()
            .filter_map(|item| {
                if let Some(s) = item.as_str() {
                    Some(s.trim().to_string())
                } else if let Some(v) = item.get("value").and_then(|v| v.as_str()) {
                    Some(v.trim().to_string())
                } else {
                    None
                }
            })
            .filter(|s| !s.is_empty())
            .collect();
        return parts.join("\n\n");
    }
    String::new()
}

/// Parse a `textDocument/completion` result — either a bare `CompletionItem[]`
/// or a `CompletionList { items: [...] }`.  Caps at 200 items.
fn parse_completion_items(result: &Value) -> Vec<CompletionItem> {
    let arr = if let Some(a) = result.as_array() {
        a
    } else if let Some(a) = result.get("items").and_then(|v| v.as_array()) {
        a
    } else {
        return Vec::new();
    };

    arr.iter()
        .take(200)
        .filter_map(|item| {
            let label = item["label"].as_str()?.to_string();
            // Prefer insertText, then textEdit.newText, then label.
            let insert_text = item
                .get("insertText")
                .and_then(|v| v.as_str())
                .or_else(|| item.pointer("/textEdit/newText").and_then(|v| v.as_str()))
                .unwrap_or(&label)
                .to_string();
            let detail = item.get("detail").and_then(|v| v.as_str()).map(str::to_string);
            Some(CompletionItem { label, insert_text, detail })
        })
        .collect()
}

fn parse_definition_result(result: &Value) -> Option<DefinitionResult> {
    // Can be `Location`, `Location[]`, or `LocationLink[]`.
    let loc = if result.is_array() {
        result.as_array()?.first()?
    } else {
        result
    };

    // LocationLink has `targetUri` / `targetRange`; Location has `uri` / `range`.
    let uri = loc
        .get("targetUri")
        .or_else(|| loc.get("uri"))
        .and_then(|v| v.as_str())?;

    let range = loc
        .get("targetSelectionRange")
        .or_else(|| loc.get("targetRange"))
        .or_else(|| loc.get("range"))?;

    let line = range["start"]["line"].as_u64()? as usize;
    let col = range["start"]["character"].as_u64()? as usize;

    Some(DefinitionResult {
        path: uri_to_path(uri),
        line,
        col,
    })
}

// ── URI helpers ────────────────────────────────────────────────────────────────

/// Convert an absolute file-system path to a `file://` URI.
pub fn path_to_uri(path: &std::path::Path) -> String {
    // On Unix paths start with `/`, giving `file:///…`.
    format!("file://{}", path.to_string_lossy())
}

fn uri_to_path(uri: &str) -> PathBuf {
    PathBuf::from(uri.trim_start_matches("file://"))
}
