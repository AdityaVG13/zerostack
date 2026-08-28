//! Stdio newline-delimited JSON-RPC loop for legacy MCP clients (CodeMode).
//!
//! `FSZeroSession` is !Send (fsqlite). A dedicated worker thread owns the
//! session for its lifetime. The stdin loop stays live so
//! `notifications/cancelled` and concurrent requests are observed. On
//! deadline/cancel we emit a structured retryable JSON-RPC error and suppress
//! late replies. The single session owner is never replaced; queued work stays
//! bounded and receives backpressure while that owner is unavailable.

use super::handler::{McpHandler, TransportProfile};
use super::request_guard::{
    REQUEST_CLEANUP_BOUND_MS, RPC_REQUEST_CANCELLED, RPC_REQUEST_DEADLINE, RequestGuard,
    deadline_error_data, matches_request_id, resolve_request_timeout_ms,
};
use super::surface::SurfaceKind;
use crate::core::FSZeroSession;
use crate::mcp_rpc::{error_response, error_response_with_data, resolve_root};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

const MAX_PENDING_REQUESTS: usize = 32;
const RPC_SERVER_BUSY: i64 = -32002;

/// `WorkerReplied` carries no payload: it exists only to wake the main loop
/// so `poll_active` runs immediately. Without it the loop sits in
/// `recv_timeout` until its timer expires, and a reply the worker produced in
/// microseconds is not noticed for up to 20ms — which was ~90% of the cost of
/// every zero.* call (zerostack-5u7).
enum Inbound {
    Line(String),
    StdinClosed,
    WorkerReplied,
}

struct WorkRequest {
    req: Value,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
    negotiated: String,
    reply: Sender<Option<Value>>,
    wake: Sender<Inbound>,
}

struct ActiveCall {
    guard: RequestGuard,
    reply_rx: Receiver<Option<Value>>,
    fail_kind: FailKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailKind {
    Deadline,
    Cancelled,
}

struct SessionWorker {
    tx: SyncSender<WorkRequest>,
}

pub fn run_stdio_server(surface: SurfaceKind) -> Result<(), String> {
    // Fail closed at the server boundary before accepting any RPC.
    super::surface::assert_server_surface_boundary(surface)?;
    let root = resolve_root()?;
    let worker = spawn_session_worker(surface, root.clone());
    let mut handler = McpHandler::new(surface, TransportProfile::StdioLegacy);

    let (line_tx, line_rx) = mpsc::channel::<Inbound>();
    let wake_tx = line_tx.clone();
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    if line_tx.send(Inbound::Line(line)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = line_tx.send(Inbound::StdinClosed);
    });

    let mut stdout = io::stdout();
    let mut pending: VecDeque<Value> = VecDeque::new();
    let mut active: Option<ActiveCall> = None;
    let mut stdin_closed = false;

    loop {
        poll_active(&mut stdout, &mut handler, &mut active);

        if active.is_none() {
            while let Some(req) = pending.pop_front() {
                if dispatch_request(
                    &mut handler,
                    &worker,
                    &mut stdout,
                    &mut active,
                    req,
                    &wake_tx,
                ) {
                    break;
                }
            }
        }

        if stdin_closed && active.is_none() && pending.is_empty() {
            break;
        }

        let wait = if let Some(act) = active.as_ref() {
            act.guard
                .remaining()
                .min(Duration::from_millis(20))
                .max(Duration::from_millis(1))
        } else if pending.is_empty() {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(1)
        };

        match line_rx.recv_timeout(wait) {
            Ok(Inbound::Line(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(req) => {
                        handle_inbound(
                            &mut handler,
                            &worker,
                            &mut stdout,
                            &mut active,
                            &mut pending,
                            req,
                            &wake_tx,
                        );
                    }
                    Err(e) => {
                        write_response(
                            &mut stdout,
                            &error_response(Value::Null, -32700, &format!("parse error: {e}")),
                        );
                    }
                }
            }
            Ok(Inbound::StdinClosed) => {
                stdin_closed = true;
                if let Some(act) = active.as_mut() {
                    act.fail_kind = FailKind::Cancelled;
                    act.guard.cancel();
                }
            }
            // The worker finished; loop around so poll_active picks the reply
            // up now rather than after the next timeout.
            Ok(Inbound::WorkerReplied) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                stdin_closed = true;
            }
        }
    }

    Ok(())
}

fn spawn_session_worker(surface: SurfaceKind, root: PathBuf) -> SessionWorker {
    let (tx, rx) = mpsc::sync_channel::<WorkRequest>(4);
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        let mut sess = FSZeroSession::with_repo_store(&root);
        sess.start_watcher_if_enabled();
        let mut handler = McpHandler::new(surface, TransportProfile::StdioLegacy);
        // Signal ready only after session construction so request deadlines
        // do not burn while with_repo_store runs.
        let _ = ready_tx.send(());
        while let Ok(work) = rx.recv() {
            handler.negotiated_version = work.negotiated;
            sess.install_request_guard(Arc::clone(&work.cancel), work.deadline);
            let response = if work.cancel.load(Ordering::SeqCst) {
                None
            } else {
                handler.handle_json(&mut sess, work.req, None)
            };
            sess.clear_request_guard();
            let _ = work.reply.send(response);
            // Wake the main loop so the reply is written immediately.
            let _ = work.wake.send(Inbound::WorkerReplied);
        }
    });
    // Bound the ready wait so a broken spawn cannot hang the stdio loop.
    match ready_rx.recv_timeout(Duration::from_secs(30)) {
        Ok(()) => {}
        Err(_) => {
            // Worker startup failure is surfaced as unavailable on dispatch.
        }
    }
    SessionWorker { tx }
}

fn poll_active(stdout: &mut io::Stdout, handler: &mut McpHandler, active: &mut Option<ActiveCall>) {
    let Some(act) = active.as_mut() else {
        return;
    };
    if act.fail_kind == FailKind::Deadline && Instant::now() >= act.guard.deadline {
        act.guard.cancel();
    }
    let expired = act.guard.is_expired();
    let kind = act.fail_kind;

    match act.reply_rx.try_recv() {
        Ok(Some(_response)) if expired => finalize_expired(stdout, active, kind),
        Ok(Some(response)) => {
            let _ = active.take();
            if let Some(pv) = response
                .pointer("/result/protocolVersion")
                .and_then(Value::as_str)
            {
                handler.negotiated_version = pv.to_string();
            }
            write_response(stdout, &response);
        }
        Ok(None) | Err(mpsc::TryRecvError::Disconnected) if expired => {
            finalize_expired(stdout, active, kind)
        }
        Ok(None) | Err(mpsc::TryRecvError::Disconnected) => {
            let _ = active.take();
        }
        Err(mpsc::TryRecvError::Empty) if expired => finalize_expired(stdout, active, kind),
        Err(mpsc::TryRecvError::Empty) => {}
    }
}

#[inline]
fn req_method(req: &Value) -> &str {
    req.get("method").and_then(Value::as_str).unwrap_or("")
}

fn handle_inbound(
    handler: &mut McpHandler,
    worker: &SessionWorker,
    stdout: &mut io::Stdout,
    active: &mut Option<ActiveCall>,
    pending: &mut VecDeque<Value>,
    req: Value,
    wake: &Sender<Inbound>,
) {
    let method = req_method(&req);
    if method == "notifications/cancelled" {
        let request_id = req
            .pointer("/params/requestId")
            .cloned()
            .or_else(|| req.pointer("/params/id").cloned());
        if let Some(request_id) = request_id {
            if let Some(act) = active
                .as_mut()
                .filter(|act| matches_request_id(&act.guard.request_id, &request_id))
            {
                act.fail_kind = FailKind::Cancelled;
                act.guard.cancel();
            } else if take_pending_by_id(pending, &request_id).is_some() {
                write_cancelled_response(stdout, request_id);
            }
        }
        return;
    }
    if req.get("id").is_none() && method.starts_with("notifications/") {
        return;
    }

    let id = req.get("id").cloned().unwrap_or(Value::Null);
    if request_id_in_flight(active, pending, &id) {
        write_busy_response(stdout, id, "duplicate in-flight stdio MCP request id");
        return;
    }

    // tools/call and handshake/list share the worker so session state stays
    // consistent; queue while another call is active.
    if active.is_some() {
        if pending.len() >= MAX_PENDING_REQUESTS {
            write_busy_response(stdout, id, "stdio MCP request queue full");
        } else {
            pending.push_back(req);
        }
        return;
    }
    let _ = dispatch_request(handler, worker, stdout, active, req, wake);
}

/// Returns true when a worker request was accepted and awaits a reply.
fn dispatch_request(
    handler: &mut McpHandler,
    worker: &SessionWorker,
    stdout: &mut io::Stdout,
    active: &mut Option<ActiveCall>,
    req: Value,
    wake: &Sender<Inbound>,
) -> bool {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req_method(&req);
    let params = req.get("params");
    let timeout = if method == "tools/call" {
        Duration::from_millis(resolve_request_timeout_ms(params))
    } else {
        Duration::from_secs(30)
    };
    let guard = RequestGuard::new(id.clone(), timeout);
    let (reply_tx, reply_rx) = mpsc::channel::<Option<Value>>();
    let work = WorkRequest {
        req,
        cancel: Arc::clone(&guard.cancel),
        deadline: guard.deadline,
        negotiated: handler.negotiated_version.clone(),
        reply: reply_tx,
        wake: wake.clone(),
    };
    match worker.tx.try_send(work) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            write_busy_response(stdout, id, "stdio MCP session worker busy");
            return false;
        }
        Err(TrySendError::Disconnected(_)) => {
            // Emit once per process: this is terminal, and a silent hard
            // failure is exactly how a session stops using the engine
            // without anything being recorded.
            static REPORTED: std::sync::Once = std::sync::Once::new();
            REPORTED.call_once(|| eprintln!(
                "fszero: stdio MCP session worker is gone; all further requests on this process will fail"));
            write_worker_gone_response(stdout, id, "stdio MCP session worker is gone (terminal)");
            return false;
        }
    }

    *active = Some(ActiveCall {
        guard,
        reply_rx,
        fail_kind: FailKind::Deadline,
    });
    true
}

fn request_id_in_flight(
    active: &Option<ActiveCall>,
    pending: &VecDeque<Value>,
    request_id: &Value,
) -> bool {
    active
        .as_ref()
        .is_some_and(|act| matches_request_id(&act.guard.request_id, request_id))
        || pending.iter().any(|req| {
            req.get("id")
                .is_some_and(|id| matches_request_id(id, request_id))
        })
}

fn take_pending_by_id(pending: &mut VecDeque<Value>, request_id: &Value) -> Option<Value> {
    let position = pending.iter().position(|req| {
        req.get("id")
            .is_some_and(|id| matches_request_id(id, request_id))
    })?;
    pending.remove(position)
}

fn write_busy_response(stdout: &mut io::Stdout, request_id: Value, message: &str) {
    write_response(
        stdout,
        &error_response_with_data(
            request_id,
            RPC_SERVER_BUSY,
            message,
            serde_json::json!({"kind":"backpressure","retryable":true,"capacity":MAX_PENDING_REQUESTS}),
        ),
    );
}

/// The session worker is gone, not merely saturated.
///
/// `Full` is genuine backpressure and clears on its own; a dropped worker
/// channel never does, because the worker owns the only session and nothing
/// respawns it. Reporting that as `retryable` makes a client retry forever
/// against a dead thread, so it is reported as a terminal failure instead.
fn write_worker_gone_response(stdout: &mut io::Stdout, request_id: Value, message: &str) {
    write_response(
        stdout,
        &error_response_with_data(
            request_id,
            RPC_SERVER_BUSY,
            message,
            serde_json::json!({"kind":"worker_gone","retryable":false}),
        ),
    );
}

fn write_cancelled_response(stdout: &mut io::Stdout, request_id: Value) {
    let message = "tools/call request cancelled";
    write_response(
        stdout,
        &error_response_with_data(
            request_id,
            RPC_REQUEST_CANCELLED,
            message,
            deadline_error_data("cancelled", message),
        ),
    );
}

fn wait_for_cleanup(reply_rx: &Receiver<Option<Value>>, bound: Duration) -> bool {
    let cleanup_deadline = Instant::now() + bound;
    while Instant::now() < cleanup_deadline {
        match reply_rx.try_recv() {
            Ok(Some(response)) => {
                let _ = response;
                return true;
            }
            Ok(None) | Err(mpsc::TryRecvError::Disconnected) => return true,
            Err(mpsc::TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
        }
    }
    false
}

fn finalize_expired(stdout: &mut io::Stdout, active: &mut Option<ActiveCall>, kind: FailKind) {
    let Some(act) = active.take() else {
        return;
    };
    act.guard.cancel();

    // Cooperative cleanup is bounded. A late reply is suppressed; the one
    // session owner remains authoritative and is never replaced.
    let _finished = wait_for_cleanup(
        &act.reply_rx,
        Duration::from_millis(REQUEST_CLEANUP_BOUND_MS),
    );

    let (code, kind_str, message) = match kind {
        FailKind::Deadline => (
            RPC_REQUEST_DEADLINE,
            "deadline",
            "tools/call deadline exceeded",
        ),
        FailKind::Cancelled => (
            RPC_REQUEST_CANCELLED,
            "cancelled",
            "tools/call request cancelled",
        ),
    };
    write_response(
        stdout,
        &error_response_with_data(
            act.guard.request_id,
            code,
            message,
            deadline_error_data(kind_str, message),
        ),
    );
}

fn write_response(stdout: &mut io::Stdout, response: &Value) {
    let _ = writeln!(stdout, "{response}");
    let _ = stdout.flush();
}
