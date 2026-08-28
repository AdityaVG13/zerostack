//! Minimal sync HTTP transport for MCP 2026-07-28 stateless requests.

use super::handler::{McpHandler, TransportProfile, tool_name_from_params};
use super::request_guard::{
    REQUEST_CLEANUP_BOUND_MS, RPC_REQUEST_CANCELLED, RPC_REQUEST_DEADLINE, RequestGuard,
    deadline_error_data, matches_request_id, resolve_request_timeout_ms,
};
use super::surface::SurfaceKind;
use super::version::PROTOCOL_RC;
use crate::core::FSZeroSession;
use crate::mcp_rpc::{error_response, error_response_with_data};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const MCP_PATH: &str = "/mcp";
const MAX_PENDING_REQUESTS: usize = 32;
const RPC_SERVER_BUSY: i64 = -32002;

struct SessionJob {
    surface: SurfaceKind,
    root: PathBuf,
    req: Value,
    protocol_version: Option<String>,
    header_method: Option<String>,
    header_name: Option<String>,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
    reply: std::sync::mpsc::Sender<Option<(u16, Value)>>,
}

struct ActiveRequest {
    request_id: Value,
    cancel: Arc<AtomicBool>,
}

struct SessionDispatcher {
    worker: SyncSender<SessionJob>,
    active: Mutex<HashMap<String, ActiveRequest>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubmitError {
    Busy,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegisterError {
    Duplicate,
    Unavailable,
}

impl SessionDispatcher {
    fn new() -> Self {
        Self {
            worker: spawn_session_worker(),
            active: Mutex::new(HashMap::new()),
        }
    }

    fn submit(&self, job: SessionJob) -> Result<(), SubmitError> {
        match self.worker.try_send(job) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(SubmitError::Busy),
            Err(TrySendError::Disconnected(_)) => Err(SubmitError::Unavailable),
        }
    }

    fn register(&self, request_id: &Value, cancel: Arc<AtomicBool>) -> Result<(), RegisterError> {
        let mut active = self.active.lock().map_err(|_| RegisterError::Unavailable)?;
        let key = request_key(request_id);
        if active.contains_key(&key) {
            return Err(RegisterError::Duplicate);
        }
        active.insert(
            key,
            ActiveRequest {
                request_id: request_id.clone(),
                cancel,
            },
        );
        Ok(())
    }

    fn unregister(&self, request_id: &Value, cancel: &Arc<AtomicBool>) {
        let Ok(mut active) = self.active.lock() else {
            return;
        };
        let key = request_key(request_id);
        if active
            .get(&key)
            .is_some_and(|registered| Arc::ptr_eq(&registered.cancel, cancel))
        {
            active.remove(&key);
        }
    }

    fn cancel(&self, request_id: &Value) -> bool {
        let Ok(active) = self.active.lock() else {
            return false;
        };
        active
            .get(&request_key(request_id))
            .is_some_and(|registered| {
                if matches_request_id(&registered.request_id, request_id) {
                    registered.cancel.store(true, Ordering::SeqCst);
                    true
                } else {
                    false
                }
            })
    }
}

fn request_key(request_id: &Value) -> String {
    serde_json::to_string(request_id).unwrap_or_else(|_| "null".to_string())
}

fn session_dispatcher() -> &'static SessionDispatcher {
    static DISPATCHER: OnceLock<SessionDispatcher> = OnceLock::new();
    DISPATCHER.get_or_init(SessionDispatcher::new)
}

/// `FSZeroSession` is intentionally single-threaded (`!Send`). One owner
/// thread holds the per-root session cache. The bounded channel applies HTTP
/// backpressure. Cancellation is cooperative; late replies are suppressed,
/// but ownership never moves to a replacement thread.
fn spawn_session_worker() -> SyncSender<SessionJob> {
    let (tx, rx) = mpsc::sync_channel::<SessionJob>(MAX_PENDING_REQUESTS);
    thread::spawn(move || {
        let mut cache: HashMap<PathBuf, FSZeroSession> = HashMap::new();
        while let Ok(job) = rx.recv() {
            if job.cancel.load(Ordering::SeqCst) {
                let _ = job.reply.send(None);
                continue;
            }
            let response = run_session_job(&mut cache, &job);
            let response = visible_reply(response, &job.cancel, job.deadline);
            let _ = job.reply.send(response);
        }
    });
    tx
}

fn visible_reply(
    response: (u16, Value),
    cancel: &AtomicBool,
    deadline: Instant,
) -> Option<(u16, Value)> {
    (!cancel.load(Ordering::SeqCst) && Instant::now() < deadline).then_some(response)
}

fn run_session_job(cache: &mut HashMap<PathBuf, FSZeroSession>, job: &SessionJob) -> (u16, Value) {
    let request_id = job.req.get("id").cloned().unwrap_or(Value::Null);
    let mut handler = McpHandler::new(job.surface, TransportProfile::HttpStateless);
    let params = job.req.get("params");
    let method = job.req.get("method").and_then(Value::as_str).unwrap_or("");
    if let Err(e) = handler.validate_http_routing(
        job.header_method.as_deref(),
        job.header_name.as_deref(),
        method,
        tool_name_from_params(params),
    ) {
        return (400, error_response(request_id, -32600, &e));
    }
    if !cache.contains_key(&job.root) {
        match FSZeroSession::try_with_repo_store(job.root.clone()) {
            Ok(sess) => {
                cache.insert(job.root.clone(), sess);
            }
            Err(e) => {
                return (
                    500,
                    error_response(
                        request_id,
                        -32603,
                        &format!("durable store open failed: {e}"),
                    ),
                );
            }
        }
    }
    let sess = cache.get_mut(&job.root).expect("session cache insert");
    sess.install_request_guard(Arc::clone(&job.cancel), job.deadline);
    let response = if job.cancel.load(Ordering::SeqCst) {
        None
    } else {
        handler.handle_json(sess, job.req.clone(), job.protocol_version.as_deref())
    };
    sess.clear_request_guard();
    let response = response
        .unwrap_or_else(|| error_response(Value::Null, -32600, "no response for notification"));
    (200, response)
}

pub struct HttpMcpServer {
    pub surface: SurfaceKind,
    pub addr: String,
}

impl HttpMcpServer {
    pub fn new(surface: SurfaceKind, addr: impl Into<String>) -> Self {
        Self {
            surface,
            addr: addr.into(),
        }
    }

    pub fn serve(self, root: PathBuf) -> Result<(), String> {
        let listener =
            TcpListener::bind(&self.addr).map_err(|e| format!("bind {}: {e}", self.addr))?;
        eprintln!(
            "fszero MCP HTTP ({}) listening on http://{}{}",
            self.surface.server_name(),
            self.addr,
            MCP_PATH
        );
        let surface = self.surface;
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("accept error: {e}");
                    continue;
                }
            };
            let root = root.clone();
            thread::spawn(move || {
                if let Err(e) = handle_connection(surface, root, stream) {
                    eprintln!("connection error: {e}");
                }
            });
        }
        Ok(())
    }
}

fn handle_connection(
    surface: SurfaceKind,
    root: PathBuf,
    mut stream: TcpStream,
) -> Result<(), String> {
    let request = read_http_request(&mut stream)?;
    if request.method != "POST" || request.path != MCP_PATH {
        write_http_response(&mut stream, 404, "text/plain", b"not found")?;
        return Ok(());
    }
    let protocol_version = request
        .header("mcp-protocol-version")
        .or_else(|| request.header("MCP-Protocol-Version"));
    if protocol_version != Some(PROTOCOL_RC) {
        write_http_response(
            &mut stream,
            400,
            "text/plain",
            format!(
                "MCP-Protocol-Version must be {PROTOCOL_RC}, got {:?}",
                protocol_version
            )
            .as_bytes(),
        )?;
        return Ok(());
    }
    let req: Value =
        serde_json::from_slice(&request.body).map_err(|e| format!("json parse: {e}"))?;
    if req.get("method").and_then(Value::as_str) == Some("notifications/cancelled") {
        if let Some(request_id) = req
            .pointer("/params/requestId")
            .or_else(|| req.pointer("/params/id"))
        {
            session_dispatcher().cancel(request_id);
        }
        write_http_response(&mut stream, 202, "application/json", b"")?;
        return Ok(());
    }

    let request_id = req.get("id").cloned().unwrap_or(Value::Null);
    let params = req.get("params");
    let timeout = if req.get("method").and_then(Value::as_str) == Some("tools/call") {
        Duration::from_millis(resolve_request_timeout_ms(params))
    } else {
        Duration::from_secs(30)
    };
    let guard = RequestGuard::new(request_id.clone(), timeout);
    let header_method = request
        .header("mcp-method")
        .or_else(|| request.header("Mcp-Method"));
    let header_name = request
        .header("mcp-name")
        .or_else(|| request.header("Mcp-Name"));
    let (reply_tx, reply_rx) = mpsc::channel();
    let job = SessionJob {
        surface,
        root,
        req,
        protocol_version: protocol_version.map(str::to_string),
        header_method: header_method.map(str::to_string),
        header_name: header_name.map(str::to_string),
        cancel: Arc::clone(&guard.cancel),
        deadline: guard.deadline,
        reply: reply_tx,
    };
    let dispatcher = session_dispatcher();
    if let Err(error) = dispatcher.register(&request_id, Arc::clone(&guard.cancel)) {
        let message = match error {
            RegisterError::Duplicate => "duplicate in-flight HTTP MCP request id",
            RegisterError::Unavailable => "HTTP MCP request registry unavailable",
        };
        let response = error_response_with_data(
            request_id,
            RPC_SERVER_BUSY,
            message,
            serde_json::json!({"kind":"backpressure","retryable":true,"capacity":MAX_PENDING_REQUESTS}),
        );
        return write_json_response(&mut stream, 503, &response);
    }
    if let Err(error) = dispatcher.submit(job) {
        dispatcher.unregister(&request_id, &guard.cancel);
        let message = match error {
            SubmitError::Busy => "HTTP MCP request queue full",
            SubmitError::Unavailable => "HTTP MCP session worker unavailable",
        };
        let response = error_response_with_data(
            request_id,
            RPC_SERVER_BUSY,
            message,
            serde_json::json!({"kind":"backpressure","retryable":true,"capacity":MAX_PENDING_REQUESTS}),
        );
        return write_json_response(&mut stream, 503, &response);
    }

    let received = reply_rx.recv_timeout(guard.remaining());
    let cancelled = guard.is_cancelled();
    let response = match received {
        Ok(Some(response)) if !cancelled && Instant::now() < guard.deadline => response,
        Ok(_) => request_guard_error(&request_id, cancelled),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let cancelled = guard.is_cancelled();
            guard.cancel();
            let _ = reply_rx.recv_timeout(Duration::from_millis(REQUEST_CLEANUP_BOUND_MS));
            request_guard_error(&request_id, cancelled)
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => (
            503,
            error_response_with_data(
                request_id.clone(),
                RPC_SERVER_BUSY,
                "HTTP MCP session worker unavailable",
                serde_json::json!({"kind":"backpressure","retryable":true,"capacity":MAX_PENDING_REQUESTS}),
            ),
        ),
    };
    dispatcher.unregister(&request_id, &guard.cancel);
    write_json_response(&mut stream, response.0, &response.1)
}

fn request_guard_error(request_id: &Value, cancelled: bool) -> (u16, Value) {
    let (code, kind, message) = if cancelled {
        (
            RPC_REQUEST_CANCELLED,
            "cancelled",
            "tools/call request cancelled",
        )
    } else {
        (
            RPC_REQUEST_DEADLINE,
            "deadline",
            "tools/call deadline exceeded",
        )
    };
    (
        200,
        error_response_with_data(
            request_id.clone(),
            code,
            message,
            deadline_error_data(kind, message),
        ),
    )
}

fn write_json_response(
    stream: &mut TcpStream,
    status: u16,
    response: &Value,
) -> Result<(), String> {
    let body = serde_json::to_vec(response).map_err(|e| e.to_string())?;
    write_http_response(stream, status, "application/json", &body)
}

struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == lower)
            .map(|(_, v)| v.as_str())
    }
}

fn read_into(
    stream: &mut TcpStream,
    chunk: &mut [u8],
    dest: &mut Vec<u8>,
) -> Result<usize, String> {
    let n = stream.read(chunk).map_err(|e| e.to_string())?;
    if n > 0 {
        dest.extend_from_slice(&chunk[..n]);
    }
    Ok(n)
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if read_into(stream, &mut chunk, &mut buf)? == 0 {
            break;
        }
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 1_048_576 {
            return Err("request headers too large".to_string());
        }
    }
    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| "malformed request".to_string())?
        + 4;
    let header_text = std::str::from_utf8(&buf[..header_end])
        .map_err(|e| e.to_string())?
        .trim_end();
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().ok_or_else(|| "empty request".to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| "missing method".to_string())?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| "missing path".to_string())?
        .to_string();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let content_length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        if read_into(stream, &mut chunk, &mut body)? == 0 {
            break;
        }
    }
    body.truncate(content_length);
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<(), String> {
    let status_text = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.write_all(body).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    Ok(())
}
