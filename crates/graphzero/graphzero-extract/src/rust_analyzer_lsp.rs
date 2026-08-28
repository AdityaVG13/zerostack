//! Live rust-analyzer LSP subprocess resolver.
//!
//! This is the concrete TypedResolver behind the typed-fusion install point: it
//! spawns a real rust-analyzer process, opens the blob as a text document, and
//! asks textDocument/definition for every referenced identifier. Each definition
//! that lands on a symbol becomes a call-accurate resolution, which is how
//! higher-order arguments, trait-qualified calls, and cross-module references --
//! all invisible to a call_expression-only structural pass -- reach the graph.
//!
//! Everything is best effort: a missing binary, a slow index, or a malformed
//! reply yields fewer resolutions, never an error in the extraction path.
//!
//! # Concurrency contract (graphzero-bw60k)
//!
//! One [`RustAnalyzerLspResolver`] owns **one** rust-analyzer child process
//! behind a [`std::sync::Mutex`]. `resolve` holds that mutex for the whole blob
//! (open_document + up to [`MAX_DEFINITION_REQUESTS`] definition RPCs).
//!
//! The store index path runs extract under rayon (`par_iter` over file chunks).
//! When this resolver is installed process-wide, every parallel blob serializes
//! on the same mutex: rayon fan-out collapses to single-threaded LSP I/O. That
//! is intentional while typed fusion is opt-in / test-only.
//!
//! **Do not** promote typed fusion to default index without one of:
//! 1. a multi-client LSP pool (N children, shard by path hash), or
//! 2. disabling rayon extract when a serial resolver is installed.
//!
//! Structural-only extract (no installed resolver) stays fully parallel.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::rust_analyzer::{ResolutionConfidence, RustAnalyzerResolvedCall};
use crate::typed_fusion::{TypedResolutions, TypedResolver};
use crate::{BlobFacts, Language};

/// Upper bound on definition requests per blob. Large files would otherwise
/// issue one round trip per identifier and dominate indexing wall time.
const MAX_DEFINITION_REQUESTS: usize = 512;

/// Rust keywords and literals that the scanner lexes as identifiers but that can
/// never name a call target.
const NON_SYMBOL_WORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

pub struct RustAnalyzerLspResolver {
    root: PathBuf,
    client: Mutex<LspClient>,
    request_timeout: Duration,
}

impl RustAnalyzerLspResolver {
    /// Spawn rust-analyzer over the project root and complete the LSP handshake.
    ///
    /// GRAPHZERO_RUST_ANALYZER_BIN overrides the executable.
    pub fn spawn(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|err| format!("project root is not readable: {err}"))?;
        let binary = std::env::var("GRAPHZERO_RUST_ANALYZER_BIN")
            .unwrap_or_else(|_| "rust-analyzer".to_string());
        let mut client = LspClient::spawn(&binary, &root)?;
        client.initialize(&root)?;
        Ok(Self {
            root,
            client: Mutex::new(client),
            request_timeout: Duration::from_secs(30),
        })
    }

    /// Absolute path a blob is opened under. Relative path hints resolve against
    /// the project root so rust-analyzer sees an in-project document.
    fn document_path(&self, path_hint: &str) -> PathBuf {
        let candidate = Path::new(path_hint);
        if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        }
    }

    fn resolve_rust(&self, path_hint: &str, content: &[u8]) -> Vec<RustAnalyzerResolvedCall> {
        let Ok(text) = std::str::from_utf8(content) else {
            return Vec::new();
        };
        let path = self.document_path(path_hint);
        let uri = path_to_uri(&path);
        let index = LineIndex::new(text);

        let mut client = match self.client.lock() {
            Ok(client) => client,
            Err(poisoned) => poisoned.into_inner(),
        };
        if client.open_document(&uri, text).is_err() {
            return Vec::new();
        }

        let mut resolved = Vec::new();
        for reference in identifier_references(text)
            .into_iter()
            .take(MAX_DEFINITION_REQUESTS)
        {
            let Some((line, character)) = index.utf16_position(reference.start) else {
                continue;
            };
            let Ok(response) = client.definition(&uri, line, character, self.request_timeout)
            else {
                continue;
            };
            let Some(target) = first_definition_target(&response) else {
                continue;
            };
            if target.uri == uri && target.start == reference.start {
                continue; // the identifier is its own definition site
            }
            let Some(name) = symbol_name_at(&target) else {
                continue;
            };
            resolved.push(RustAnalyzerResolvedCall::new(
                reference.start as u32,
                reference.end as u32,
                name,
                ResolutionConfidence::Exact,
            ));
        }

        let _ = client.close_document(&uri);
        resolved
    }
}

impl TypedResolver for RustAnalyzerLspResolver {
    fn resolve(
        &self,
        path_hint: Option<&str>,
        content: &[u8],
        facts: &BlobFacts,
    ) -> TypedResolutions {
        let Some(path_hint) = path_hint else {
            return TypedResolutions::default();
        };
        if facts.language != Language::Rust {
            return TypedResolutions::default();
        }
        TypedResolutions {
            rust_calls: self.resolve_rust(path_hint, content),
            typescript_edges: Vec::new(),
        }
    }
}

impl Drop for RustAnalyzerLspResolver {
    fn drop(&mut self) {
        let mut client = match self.client.lock() {
            Ok(client) => client,
            Err(poisoned) => poisoned.into_inner(),
        };
        client.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Identifier scanning
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IdentifierRef {
    start: usize,
    end: usize,
}

/// Byte spans of identifiers that could name a symbol defined elsewhere.
///
/// Line comments, block comments, string literals, attributes, keywords, and the
/// names in definition positions (fn NAME, struct NAME, ...) are all skipped:
/// asking rust-analyzer to resolve those spends a round trip to learn nothing.
fn identifier_references(text: &str) -> Vec<IdentifierRef> {
    let bytes = text.as_bytes();
    let mut refs = Vec::new();
    let mut i = 0usize;
    let mut previous_word: Option<&str> = None;

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        if b == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
            continue;
        }
        if b == b'#' && bytes.get(i + 1) == Some(&b'[') {
            let mut depth = 0usize;
            while i < bytes.len() {
                match bytes[i] {
                    b'[' => depth += 1,
                    b']' => {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            continue;
        }
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &text[start..i];
            let is_definition_name = matches!(
                previous_word,
                Some("fn" | "struct" | "enum" | "trait" | "mod" | "type" | "const" | "static")
            );
            if !NON_SYMBOL_WORDS.contains(&word) && !is_definition_name {
                refs.push(IdentifierRef { start, end: i });
            }
            previous_word = Some(word);
            continue;
        }
        if !b.is_ascii_whitespace() {
            previous_word = None;
        }
        i += 1;
    }

    refs
}

// ---------------------------------------------------------------------------
// Position mapping
// ---------------------------------------------------------------------------

struct LineIndex<'a> {
    text: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    fn new(text: &'a str) -> Self {
        let mut line_starts = vec![0usize];
        line_starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        );
        Self { text, line_starts }
    }

    /// LSP positions are zero-based lines with UTF-16 code-unit columns.
    fn utf16_position(&self, offset: usize) -> Option<(u32, u32)> {
        if offset > self.text.len() {
            return None;
        }
        let line = self.line_starts.partition_point(|start| *start <= offset) - 1;
        let line_start = self.line_starts[line];
        let column = self
            .text
            .get(line_start..offset)?
            .chars()
            .map(char::len_utf16)
            .sum::<usize>();
        Some((line as u32, column as u32))
    }
}

fn path_to_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(byte as char);
            }
            other => uri.push_str(&format!("%{other:02X}")),
        }
    }
    uri
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(decoded).ok()?))
}

// ---------------------------------------------------------------------------
// Definition responses
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
struct DefinitionTarget {
    uri: String,
    line: u32,
    character: u32,
    /// Byte offset of the definition inside its own file, when readable.
    start: usize,
}

/// rust-analyzer answers textDocument/definition with a Location, an array of
/// Location, or an array of LocationLink. All three shapes are accepted.
fn first_definition_target(response: &Value) -> Option<DefinitionTarget> {
    let entry = match response {
        Value::Array(items) => items.first()?,
        other => other,
    };
    let (uri, range) = match (entry.get("uri"), entry.get("targetUri")) {
        (Some(uri), _) => (uri, entry.get("range")?),
        (None, Some(uri)) => (
            uri,
            entry
                .get("targetSelectionRange")
                .or_else(|| entry.get("targetRange"))?,
        ),
        _ => return None,
    };
    let uri = uri.as_str()?.to_string();
    let start = range.get("start")?;
    let line = start.get("line")?.as_u64()? as u32;
    let character = start.get("character")?.as_u64()? as u32;
    let byte_start = uri_to_path(&uri)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| byte_offset(&text, line, character))
        .unwrap_or(usize::MAX);
    Some(DefinitionTarget {
        uri,
        line,
        character,
        start: byte_start,
    })
}

fn byte_offset(text: &str, line: u32, character: u32) -> Option<usize> {
    let line_start = if line == 0 {
        0
    } else {
        text.bytes()
            .enumerate()
            .filter(|(_, b)| *b == b'\n')
            .map(|(i, _)| i + 1)
            .nth(line as usize - 1)?
    };
    let mut offset = line_start;
    let mut remaining = character as usize;
    for ch in text.get(line_start..)?.chars() {
        if remaining == 0 {
            break;
        }
        remaining = remaining.checked_sub(ch.len_utf16())?;
        offset += ch.len_utf8();
    }
    Some(offset)
}

/// Identifier text at a definition site, read out of the target file.
fn symbol_name_at(target: &DefinitionTarget) -> Option<String> {
    let path = uri_to_path(&target.uri)?;
    let text = std::fs::read_to_string(&path).ok()?;
    let start = byte_offset(&text, target.line, target.character)?;
    let bytes = text.as_bytes();
    let mut end = start;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    (end > start).then(|| text[start..end].to_string())
}

// ---------------------------------------------------------------------------
// Minimal LSP client over stdio
// ---------------------------------------------------------------------------

/// Owner session and worker generation bound to every rust-analyzer child.
/// Every spawn captures these exact values and every teardown signal must
/// present them again (see `signal_graceful_for`).
const RA_OWNER_SESSION: &str = "graphzero:extract:rust-analyzer";
const RA_WORKER_GENERATION: u64 = 0;

struct LspClient {
    child: graphzero_types::child_identity::VerifiedChild,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    next_id: i64,
    open: BTreeSet<String>,
}

impl LspClient {
    fn spawn(binary: &str, root: &Path) -> Result<Self, String> {
        let mut command = Command::new(binary);
        command
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // Spawn inside an isolated process tree (Unix process group / Windows
        // Job Object with kill-on-close) and capture the identity before any
        // LSP work is accepted.
        let (child, stdin, stdout) = graphzero_types::child_identity::VerifiedChild::spawn_tree(
            command,
            RA_OWNER_SESSION,
            RA_WORKER_GENERATION,
        )
        .map_err(|err| format!("failed to spawn {binary}: {err}"))?;
        let stdin = stdin.ok_or("rust-analyzer stdin unavailable")?;
        let stdout = stdout.ok_or("rust-analyzer stdout unavailable")?;
        let (sender, messages) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(message) = read_message(&mut reader) {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            messages,
            next_id: 0,
            open: BTreeSet::new(),
        })
    }

    fn send(&mut self, payload: &Value) -> Result<(), String> {
        let body = serde_json::to_string(payload).map_err(|err| err.to_string())?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len())
            .and_then(|()| self.stdin.flush())
            .map_err(|err| format!("rust-analyzer stdin write failed: {err}"))
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        self.await_response(id, timeout)
    }

    /// Drain notifications and unrelated traffic until the matching response id
    /// arrives or the deadline passes.
    fn await_response(&mut self, id: i64, timeout: Duration) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!("timed out waiting for response {id}"));
            }
            match self.messages.recv_timeout(remaining) {
                Ok(message) => {
                    if message.get("id").and_then(Value::as_i64) == Some(id) {
                        if let Some(error) = message.get("error") {
                            return Err(error.to_string());
                        }
                        return Ok(message.get("result").cloned().unwrap_or(Value::Null));
                    }
                    // Server-to-client requests must be answered or rust-analyzer
                    // stalls waiting on them.
                    if let (Some(server_id), Some(_)) = (message.get("id"), message.get("method")) {
                        let reply =
                            json!({"jsonrpc": "2.0", "id": server_id, "result": Value::Null});
                        self.send(&reply)?;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!("timed out waiting for response {id}"));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("rust-analyzer exited".to_string());
                }
            }
        }
    }

    fn initialize(&mut self, root: &Path) -> Result<(), String> {
        let uri = path_to_uri(root);
        self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": uri,
                "capabilities": {
                    "textDocument": {
                        "definition": {"linkSupport": true},
                        "synchronization": {"dynamicRegistration": false}
                    },
                    "window": {"workDoneProgress": true}
                },
                "workspaceFolders": [{"uri": uri, "name": "graphzero"}],
            }),
            Duration::from_secs(60),
        )?;
        self.send(&json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}))
    }

    fn open_document(&mut self, uri: &str, text: &str) -> Result<(), String> {
        if !self.open.insert(uri.to_string()) {
            return Ok(());
        }
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": uri, "languageId": "rust", "version": 1, "text": text
            }},
        }))
    }

    fn close_document(&mut self, uri: &str) -> Result<(), String> {
        if !self.open.remove(uri) {
            return Ok(());
        }
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didClose",
            "params": {"textDocument": {"uri": uri}},
        }))
    }

    fn definition(
        &mut self,
        uri: &str,
        line: u32,
        character: u32,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.request(
            "textDocument/definition",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            }),
            timeout,
        )
    }

    fn shutdown(&mut self) {
        let _ = self.request("shutdown", Value::Null, Duration::from_secs(5));
        let _ = self.send(&json!({"jsonrpc": "2.0", "method": "exit"}));
        // Verified teardown through the owned, unreaped child handle: the
        // owner/generation binding and start identity are checked inside the
        // signal action itself; the handle pins the pid until reap, so no
        // unrelated replacement process can be signaled.
        let _ = self.child.signal_graceful_for(
            RA_OWNER_SESSION,
            RA_WORKER_GENERATION,
            Duration::from_secs(5),
        );
        let _ = self.child.revoke();
    }
}

fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            return None;
        }
        let trimmed = header.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0u8; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}
