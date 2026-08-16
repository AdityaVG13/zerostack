//! Harness-owned stdio MCP carrier.
//!
//! This is the product surface for Grok, Cursor, Claude, and any other
//! MCP host: the host starts `zsx mcp` as a child and owns stdin. It is
//! not a sidecar, not a launchd service, and it never detaches. Idle state
//! is a blocking stdin read (zero CPU). The process exits when stdin
//! hits EOF (host closed the session) or when the parent pid changes
//! (host crashed and we were reparented). Work is one CodeMode plan at
//! a time, bounded by the request timeout. Stores stay warm across
//! `zero_execute` calls in this process only.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use zsx_core::{
    ZsxSession, fs_write_grant_count_for_plan, harness_fs_write_grants,
};

use crate::exec::{DEFAULT_TIMEOUT_MS, ZSX_PROTOCOL};
use crate::reexec;

const PROTOCOL: &str = "2024-11-05";
const SERVER_NAME: &str = "zerostack-zsx";
const MAX_LIVE_SESSIONS: usize = 8;
const NEXT_REQUEST_ID_FILE: &str = "mcp-next-request-id";

/// Session id must change when a cached live session is evicted and
/// recreated. `pid + root` alone collides with leftover attempt journals
/// at `g1/r1/<seed>` and surfaces as `already_terminal`.
fn mint_mcp_session_id(root: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut hasher);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!(
        "zsx-mcp-{:x}-{:x}-{:x}",
        std::process::id(),
        hasher.finish(),
        nonce
    )
}

fn next_request_id_path(state_root: &Path) -> PathBuf {
    state_root.join(NEXT_REQUEST_ID_FILE)
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

/// Persist lives under `{root}/.zerostack`. A planted symlink there is a
/// write-through primitive (`std::fs::write` follows). Fail closed, matching
/// zero-store's refusal of a symlinked `.zerostack` directory marker.
fn persist_paths_trusted(state_root: &Path) -> bool {
    !is_symlink(state_root) && !is_symlink(&next_request_id_path(state_root))
}

fn load_next_request_id(state_root: &Path) -> u64 {
    if !persist_paths_trusted(state_root) {
        return 1;
    }
    std::fs::read_to_string(next_request_id_path(state_root))
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .filter(|value| *value >= 1)
        .unwrap_or(1)
}

fn store_next_request_id(state_root: &Path, next: u64) {
    if std::fs::create_dir_all(state_root).is_err() {
        return;
    }
    if !persist_paths_trusted(state_root) {
        return;
    }
    let _ = std::fs::write(next_request_id_path(state_root), format!("{next}\n"));
}

enum FrameKind {
    Ndjson,
    Lsp,
}

struct LiveSession {
    session: ZsxSession,
    last_used: Instant,
    next_request_id: u64,
}

enum CacheHit {
    Live,
    Joining,
    Finished,
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
        if !root.is_dir() {
            return Err(format!("root is not a directory: {}", root.display()));
        }
        let cached_state = self.sessions.get(&root).map(|live| {
            if live.session.shutdown_in_progress() {
                if live.session.shutdown().is_ok() {
                    CacheHit::Finished
                } else {
                    CacheHit::Joining
                }
            } else {
                CacheHit::Live
            }
        });
        match cached_state {
            Some(CacheHit::Joining) => {
                return Err(format!(
                    "root {} is shutting down; worker has not stopped",
                    root.display()
                ));
            }
            Some(CacheHit::Finished) => {
                self.sessions.remove(&root);
            }
            Some(CacheHit::Live) => {
                let live = self.sessions.get_mut(&root).expect("live cache hit");
                live.last_used = Instant::now();
                return Ok(live);
            }
            None => {}
        }
        while self.sessions.len() >= MAX_LIVE_SESSIONS {
            let oldest = self
                .sessions
                .iter()
                .min_by_key(|(_, live)| live.last_used)
                .map(|(path, _)| path.clone());
            if let Some(path) = oldest {
                if let Some(live) = self.sessions.remove(&path) {
                    if live.session.shutdown().is_err() {
                        self.sessions.insert(path, live);
                        break;
                    }
                }
            } else {
                break;
            }
        }
        if self.sessions.len() >= MAX_LIVE_SESSIONS {
            return Err(format!(
                "cannot open root {}: live session cap {MAX_LIVE_SESSIONS} is full (a shutdown is still joining)",
                root.display()
            ));
        }
        let session_id = mint_mcp_session_id(&root);
        let state_root = root.join(".zerostack");
        if is_symlink(&state_root) {
            return Err(format!(
                "refusing symlinked state root {}",
                state_root.display()
            ));
        }
        let next_request_id = load_next_request_id(&state_root);
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
                next_request_id,
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
        let root = root.canonicalize().map_err(|error| {
            format!("cannot canonicalize {}: {error}", root.display())
        })?;
        let root_text = root.to_string_lossy().into_owned();
        let state_root = root.join(".zerostack");
        let live = self.session_for(root)?;
        let request_id = live.next_request_id;
        live.next_request_id = live.next_request_id.saturating_add(1);
        store_next_request_id(&state_root, live.next_request_id);
        let grants = harness_fs_write_grants(
            &root_text,
            1,
            request_id,
            fs_write_grant_count_for_plan(plan),
        );
        let result = live
            .session
            .execute_with_approvals(
                1,
                request_id,
                plan,
                Duration::from_millis(timeout_ms),
                grants,
            )
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
            "description": "Reports that this zsx mcp process is ready (pid, ppid, lifetime:harness-stdio). No child is spawned. The process exits on stdin EOF or parent death. Harnesses must not wrap, detach, or register engine MCP.",
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
        "ppid": current_ppid(),
        "idle": "blocking-stdin",
        "lifetime": "harness-stdio",
        "image": reexec::image_payload(),
    })
}

fn current_ppid() -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::process::parent_id()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Exit the process if the harness parent dies and we are reparented.
/// Polls in-process; this is not a sidecar and has no extra pid.
fn install_parent_death_exit() {
    #[cfg(unix)]
    {
        let parent = current_ppid();
        if parent <= 1 {
            return;
        }
        let _ = std::thread::Builder::new()
            .name("zsx-mcp-parent-death".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(50));
                let now = current_ppid();
                if now == 1 || now != parent {
                    std::process::exit(0);
                }
            });
    }
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
                    "lifetime": "harness-stdio",
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
                let plan = arguments.get("plan").and_then(Value::as_str).unwrap_or("");
                let root = arguments
                    .get("root")
                    .and_then(Value::as_str)
                    .map(PathBuf::from);
                let timeout_ms = match arguments.get("timeout_ms") {
                    None => DEFAULT_TIMEOUT_MS,
                    Some(Value::Number(n)) => match n.as_u64() {
                        Some(v) => v,
                        None => {
                            return Some(json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": tool_result("timeout_ms must be a positive integer".into(), true),
                            }));
                        }
                    },
                    Some(_) => {
                        return Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": tool_result("timeout_ms must be a positive integer".into(), true),
                        }));
                    }
                };
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

fn header_key(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    let (key, _) = text.split_once(':')?;
    let key = key.trim();
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return None;
    }
    Some(key)
}

fn is_lsp_opener(bytes: &[u8]) -> bool {
    header_key(bytes).is_some_and(|key| key.eq_ignore_ascii_case("content-length"))
}

fn is_lsp_header_line(bytes: &[u8]) -> bool {
    header_key(bytes).is_some()
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
    // Only Content-Length/Content-Type start an LSP frame. A stray log
    // line must not trap the server waiting for a blank header terminator.
    if !is_lsp_opener(trimmed) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid mcp frame: {}", String::from_utf8_lossy(trimmed)),
        ));
    }
    let mut headers = first;
    let mut header_lines = 1usize;
    loop {
        if header_lines > 16 || headers.len() > 4096 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "mcp header block too large",
            ));
        }
        let mut line = Vec::new();
        let n = stdin.read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(None);
        }
        headers.extend_from_slice(&line);
        header_lines += 1;
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        if !is_lsp_header_line(strip_crlf(&line)) && line != b"\r\n" && line != b"\n" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid mcp header line",
            ));
        }
    }
    let length = content_length(&headers)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    if length > 8 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "mcp body exceeds 8MiB",
        ));
    }
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

const DETACHED_ENV_REFUSALS: &[(&str, &str)] = &[
    ("LISTEN_PID", "systemd socket activation"),
    ("LISTEN_FDS", "systemd socket activation"),
    ("NOTIFY_SOCKET", "systemd notify"),
    ("INVOCATION_ID", "systemd service"),
];

const OURS_IN_LAUNCHD_NAME: &[&str] = &["zsx", "zerostack", "fszero", "graphzero", "tokenzero"];

fn launchd_service_is_ours(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    OURS_IN_LAUNCHD_NAME
        .iter()
        .any(|needle| lower.contains(needle))
}

fn stdin_is_harness_stdio() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        match std::fs::metadata("/dev/fd/0") {
            Ok(meta) => {
                let kind = meta.file_type();
                kind.is_fifo() || kind.is_socket()
            }
            Err(_) => true,
        }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Why this process must not serve. `None` means a harness owns stdio.
fn detached_launch_reason() -> Option<String> {
    if let Ok(name) = std::env::var("XPC_SERVICE_NAME") {
        if launchd_service_is_ours(&name) {
            return Some(format!(
                "refuses LaunchAgent/XPC ({name}). Harness must exec bin/zsx mcp on stdio."
            ));
        }
    }
    for (key, why) in DETACHED_ENV_REFUSALS {
        if std::env::var_os(key).is_some() {
            return Some(format!(
                "refuses {why} ({key}). Harness must exec bin/zsx mcp on stdio."
            ));
        }
    }
    if current_ppid() <= 1 {
        return Some(
            "refuses to start as an orphan (ppid<=1). Harness must own this process.".into(),
        );
    }
    if !stdin_is_harness_stdio() {
        return Some(
            "refuses a detached stdin (not a pipe/socket). Do not LaunchAgent or redirect from /dev/null.".into(),
        );
    }
    None
}

fn exit_if_detached_launch() {
    if let Some(reason) = detached_launch_reason() {
        eprintln!("zsx mcp: {reason}");
        std::process::exit(2);
    }
}

/// Serve MCP on stdin/stdout until EOF. Returns after the host hangs up.
///
/// Do not hold `stdout.lock()` across `handle`: execute can `println!` /
/// engine-log, and a held stdout lock deadlocks the only thread.
pub fn serve(default_root: PathBuf) -> io::Result<()> {
    exit_if_detached_launch();
    reexec::capture_running_image();
    install_parent_death_exit();
    let mut host = McpHost::new(default_root);
    let mut stdin = io::BufReader::new(io::stdin());
    loop {
        match detect_and_read(&mut stdin) {
            Ok(None) => break,
            Ok(Some((kind, message))) => {
                if let Some(response) = handle(&mut host, &message) {
                    let mut stdout = io::stdout();
                    write_frame(&mut stdout, &kind, &response)?;
                }
                reexec::reexec_if_plugin_bin_changed();
            }
            Err(error) => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": {"code": -32700, "message": error.to_string()},
                });
                let mut stdout = io::stdout();
                write_frame(&mut stdout, &FrameKind::Ndjson, &response)?;
                reexec::reexec_if_plugin_bin_changed();
            }
        }
    }
    host.shutdown();
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/rust/zsx/mcp_rmja_session_identity_tests.rs"]
mod rmja_session_identity_tests;
