//! Harness-owned stdio MCP carrier.
//!
//! This is the product surface for Grok, Cursor, Claude, and any other
//! MCP host: the host starts `zsx mcp` and kills it when the session
//! ends. It is not a sidecar. Idle state is a blocking stdin read
//! (zero CPU). Work is one CodeMode plan at a time, bounded by the
//! request timeout. Stores stay warm across `zero_execute` calls in
//! this process.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zsx_core::ZsxSession;

use crate::exec::{DEFAULT_TIMEOUT_MS, ZSX_PROTOCOL};

const PROTOCOL: &str = "2024-11-05";
const SERVER_NAME: &str = "zerostack-zsx";
const MAX_LIVE_SESSIONS: usize = 4;

enum FrameKind {
    Ndjson,
    Lsp,
}

struct LiveSession {
    session: ZsxSession,
    last_used: Instant,
    next_request_id: u64,
}

/// In-process session cache keyed by canonical root.
pub struct McpHost {
    default_root: PathBuf,
    sessions: HashMap<PathBuf, LiveSession>,
}

impl McpHost {
    pub fn new(default_root: PathBuf) -> Self {
        Self {
            default_root,
            sessions: HashMap::new(),
        }
    }

    fn session_for(&mut self, root: PathBuf) -> Result<&mut LiveSession, String> {
        let root = root
            .canonicalize()
            .map_err(|error| format!("cannot canonicalize {}: {error}", root.display()))?;
        if self.sessions.contains_key(&root) {
            let live = self.sessions.get_mut(&root).expect("just checked");
            live.last_used = Instant::now();
            return Ok(live);
        }
        while self.sessions.len() >= MAX_LIVE_SESSIONS {
            let oldest = self
                .sessions
                .iter()
                .min_by_key(|(_, live)| live.last_used)
                .map(|(path, _)| path.clone());
            if let Some(path) = oldest {
                if let Some(live) = self.sessions.remove(&path) {
                    let _ = live.session.shutdown();
                }
            } else {
                break;
            }
        }
        let session_id = format!("zsx-mcp-{:x}", std::process::id());
        let state_root = root.join(".zerostack");
        let session = ZsxSession::builder(root.clone())
            .with_state_root(state_root)
            .with_session_id(session_id)
            .build_canonical()
            .map_err(|error| error.to_string())?;
        self.sessions.insert(
            root.clone(),
            LiveSession {
                session,
                last_used: Instant::now(),
                next_request_id: 1,
            },
        );
        Ok(self.sessions.get_mut(&root).expect("just inserted"))
    }

    pub fn zero_execute(
        &mut self,
        plan: &str,
        root: Option<PathBuf>,
        timeout_ms: u64,
    ) -> Result<Value, String> {
        let plan = plan.trim();
        if plan.is_empty() {
            return Err("zero_execute requires plan".into());
        }
        if timeout_ms == 0 {
            return Err("timeout_ms must be nonzero".into());
        }
        let root = root.unwrap_or_else(|| self.default_root.clone());
        let live = self.session_for(root)?;
        let request_id = live.next_request_id;
        live.next_request_id = live.next_request_id.saturating_add(1);
        let result = live
            .session
            .execute(1, request_id, plan, Duration::from_millis(timeout_ms))
            .map_err(|error| error.to_string())?;
        Ok(json!({
            "protocol": ZSX_PROTOCOL,
            "ok": true,
            "generation": result.generation,
            "request_id": result.request_id,
            "result": result.value,
        }))
    }

    pub fn shutdown(&mut self) {
        for (_, live) in self.sessions.drain() {
            let _ = live.session.shutdown();
        }
    }
}

impl Drop for McpHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn tools() -> Value {
    json!([
        {
            "name": "zero_execute",
            "description": "Run one ZeroStack CodeMode plan through in-process zsx (FSZero + GraphZero + TokenZero). Plan is JS, e.g. return await zero.fs.compound(\"read\", {path: \"README.md\"});",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "plan": {
                        "type": "string",
                        "description": "CodeMode JavaScript plan"
                    },
                    "root": {
                        "type": "string",
                        "description": "Authorized engine root. Defaults to the process cwd."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Execution timeout in milliseconds (default 30000)"
                    }
                },
                "required": ["plan"]
            }
        },
        {
            "name": "zero_wait",
            "description": "Reports that the live zsx MCP process is ready. No child process is spawned.",
            "inputSchema": {"type": "object", "properties": {}}
        }
    ])
}

fn tool_result(text: String, is_error: bool) -> Value {
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    })
}

fn zero_wait_payload() -> Value {
    let exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "zsx".into());
    json!({
        "zsx": exe,
        "present": true,
        "mcp": true,
        "pid": std::process::id(),
        "idle": "blocking-stdin",
    })
}

pub fn handle(host: &mut McpHost, message: &Value) -> Option<Value> {
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let id = message.get("id").cloned();
    if method == "initialize" {
        return Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": PROTOCOL,
                "capabilities": {"tools": {}},
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }
        }));
    }
    if method == "notifications/initialized" || method == "initialized" {
        return None;
    }
    if method == "tools/list" {
        return Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {"tools": tools()},
        }));
    }
    if method == "tools/call" {
        let params = message.get("params").cloned().unwrap_or(json!({}));
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
        let result = match name {
            "zero_execute" => {
                let plan = arguments
                    .get("plan")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let root = arguments
                    .get("root")
                    .and_then(Value::as_str)
                    .map(PathBuf::from);
                let timeout_ms = arguments
                    .get("timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(DEFAULT_TIMEOUT_MS);
                match host.zero_execute(plan, root, timeout_ms) {
                    Ok(value) => tool_result(value.to_string(), false),
                    Err(error) => tool_result(error, true),
                }
            }
            "zero_wait" => tool_result(
                serde_json::to_string_pretty(&zero_wait_payload()).unwrap_or_default(),
                false,
            ),
            other => tool_result(format!("unknown tool {other}"), true),
        };
        return Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }));
    }
    if method == "ping" {
        return Some(json!({"jsonrpc": "2.0", "id": id, "result": {}}));
    }
    if id.is_none() {
        return None;
    }
    Some(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": -32601, "message": format!("method not found: {method}")},
    }))
}

fn detect_and_read(stdin: &mut impl BufRead) -> io::Result<Option<(FrameKind, Value)>> {
    let mut first = Vec::new();
    let n = stdin.read_until(b'\n', &mut first)?;
    if n == 0 {
        return Ok(None);
    }
    let trimmed = strip_crlf(&first);
    if trimmed.first() == Some(&b'{') {
        let value = serde_json::from_slice(trimmed).map_err(io::Error::other)?;
        return Ok(Some((FrameKind::Ndjson, value)));
    }
    let mut headers = first;
    loop {
        let mut line = Vec::new();
        let n = stdin.read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(None);
        }
        headers.extend_from_slice(&line);
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }
    let length = content_length(&headers).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length")
    })?;
    let mut body = vec![0_u8; length];
    stdin.read_exact(&mut body)?;
    let value = serde_json::from_slice(&body).map_err(io::Error::other)?;
    Ok(Some((FrameKind::Lsp, value)))
}

fn strip_crlf(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    if end > 0 && bytes[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && bytes[end - 1] == b'\r' {
        end -= 1;
    }
    &bytes[..end]
}

fn content_length(headers: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(headers).ok()?;
    for line in text.split(['\n', '\r']) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once(':')?;
        if key.eq_ignore_ascii_case("content-length") {
            return value.trim().parse().ok();
        }
    }
    None
}

fn write_frame(stdout: &mut impl Write, kind: &FrameKind, value: &Value) -> io::Result<()> {
    let raw = serde_json::to_vec(value)?;
    match kind {
        FrameKind::Ndjson => {
            stdout.write_all(&raw)?;
            stdout.write_all(b"\n")?;
        }
        FrameKind::Lsp => {
            write!(stdout, "Content-Length: {}\r\n\r\n", raw.len())?;
            stdout.write_all(&raw)?;
        }
    }
    stdout.flush()
}

/// Serve MCP on stdin/stdout until EOF. Returns after the host hangs up.
pub fn serve(default_root: PathBuf) -> io::Result<()> {
    let mut host = McpHost::new(default_root);
    let mut stdin = io::BufReader::new(io::stdin().lock());
    let mut stdout = io::stdout().lock();
    let mut kind;
    loop {
        let Some((detected, message)) = detect_and_read(&mut stdin)? else {
            break;
        };
        kind = detected;
        if let Some(response) = handle(&mut host, &message) {
            write_frame(&mut stdout, &kind, &response)?;
        }
    }
    host.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_tools_list_does_not_open_a_store() {
        let mut host = McpHost::new(std::env::temp_dir());
        let response = handle(
            &mut host,
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        )
        .expect("reply");
        let names: Vec<&str> = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["zero_execute", "zero_wait"]);
        assert!(host.sessions.is_empty(), "list must not open a store");
    }

    #[test]
    fn content_length_parses_case_insensitive() {
        assert_eq!(
            content_length(b"Content-Length: 12\r\n\r\n"),
            Some(12)
        );
        assert_eq!(content_length(b"content-length: 7\n\n"), Some(7));
    }

    #[test]
    fn ndjson_round_trip_keeps_one_line() {
        let value = json!({"ok": true});
        let mut buf = Vec::new();
        write_frame(&mut buf, &FrameKind::Ndjson, &value).unwrap();
        assert_eq!(buf.last().copied(), Some(b'\n'));
        assert_eq!(buf.iter().filter(|b| **b == b'\n').count(), 1);
    }
}
