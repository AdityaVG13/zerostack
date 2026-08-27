//! Shared planner-free GraphZero raw-worker stdio adapter.
//!
//! Line-delimited JSON framed protocol over stdin/stdout.
//!
//! v1 (zerostack.surface handshake, retained unchanged):
//!   {"kind":"handshake","request":{...}}
//!   {"kind":"call","op":"...","args":{...},"request_id":"...",
//!    "cancelled":false,"deadline_exceeded":false}
//!
//! Canonical v2 (zerostack.raw_worker, additive): nested
//! handshake/call/cancel/shutdown frames with protocol/root/session/engine/
//! digest binding, <=1MiB bounded NDJSON enforced before parse/emit, typed
//! result metadata (effect/approval/revert/ownership/trace), expired-deadline
//! rejection, and truthful capabilities (no active-cancellation claim; the
//! sidecar may kill the process). A session enters v2 when a frame carries
//! request.protocol_version == "zerostack.raw_worker" and stays v2.
//!
//! Per-call cancelled / deadline_exceeded (v1) set EngineContext preflight for
//! that call only (typed Cancelled / DeadlineExceeded).
//!
//! Env: GRAPHZERO_REPO, GRAPHZERO_STORE (defaults: ., ./.graphzero),
//!      ZEROSTACK_SESSION_ID (v2 session binding), ZEROSTACK_WORKER_REVISION
//!      (v2 worker revision; falls back to the crate version).

use crate::dispatcher::{AdapterKind, EngineContext};
use crate::surface_handshake::{
    PrivateRawWorker, SelectedSurface, WorkerRequestFrame, WorkerResponseFrame,
    raw_worker::{self, RawWorker},
};
use serde_json::Value;
use std::env;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

enum BoundedFrame {
    Eof,
    Line(Vec<u8>),
    TooLarge,
}

/// Read and fully drain one NDJSON frame without retaining more than max+1
/// bytes. This enforces the frame bound before either v1 or v2 JSON parsing.
fn read_bounded_frame<R: BufRead>(reader: &mut R, maximum: usize) -> io::Result<BoundedFrame> {
    let mut line = Vec::with_capacity(4096);
    let mut too_large = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() && !too_large {
                return Ok(BoundedFrame::Eof);
            }
            return Ok(if too_large || line.len() > maximum {
                BoundedFrame::TooLarge
            } else {
                BoundedFrame::Line(line)
            });
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if !too_large {
            if line.len().saturating_add(take) > maximum.saturating_add(1) {
                too_large = true;
                line.clear();
            } else {
                line.extend_from_slice(&available[..take]);
            }
        }
        reader.consume(take);
        if newline.is_some() {
            let content = line.strip_suffix(b"\n").unwrap_or(&line);
            let content_len = content.strip_suffix(b"\r").unwrap_or(content).len();
            return Ok(if too_large || content_len > maximum {
                BoundedFrame::TooLarge
            } else {
                BoundedFrame::Line(line)
            });
        }
    }
}

/// Run one bounded stdio session. The caller owns argv parsing and metadata probes.
pub fn run_stdio(repo_override: Option<PathBuf>) -> i32 {
    let repo = repo_override.unwrap_or_else(|| {
        PathBuf::from(env::var("GRAPHZERO_REPO").unwrap_or_else(|_| ".".into()))
    });
    let store = env::var_os("GRAPHZERO_STORE")
        .map(PathBuf::from)
        .unwrap_or_else(|| graphzero_store::resolve_graphzero_store_root(&repo));
    let surface = match env::var("GRAPHZERO_SURFACE").as_deref() {
        Ok("codemode") => SelectedSurface::Codemode,
        _ => SelectedSurface::Mcp,
    };
    let mut worker = PrivateRawWorker::for_client_native(surface);
    let session_id = env::var("ZEROSTACK_SESSION_ID").unwrap_or_else(|_| "gz-raw-worker".into());
    let mut protocol_worker = RawWorker::new(repo.to_string_lossy().into_owned(), session_id);
    let mut v2_mode = false;
    let mut saw_error = false;
    let ctx = EngineContext::for_paths(repo, store, AdapterKind::PrivateWorker);
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut stdout = io::stdout();
    loop {
        let line = match read_bounded_frame(&mut input, raw_worker::DEFAULT_MAX_FRAME_BYTES) {
            Ok(BoundedFrame::Eof) => break,
            Ok(BoundedFrame::TooLarge) => {
                saw_error = true;
                let out = protocol_worker
                    .handle_line(&ctx, &vec![b'x'; raw_worker::DEFAULT_MAX_FRAME_BYTES + 1]);
                let _ = stdout.write_all(&out);
                let _ = stdout.flush();
                continue;
            }
            Ok(BoundedFrame::Line(line)) => line,
            Err(e) => {
                saw_error = true;
                let _ = writeln!(stdout, "{{\"kind\":\"error\",\"error\":{}}}", e);
                break;
            }
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        // Canonical v2 path: bounded NDJSON with nested frames. Detection: the
        // v2 handshake carries request.protocol_version; the session stays v2.
        let looks = v2_mode
            || serde_json::from_slice::<Value>(&line)
                .ok()
                .and_then(|frame| {
                    frame
                        .get("request")?
                        .get("protocol_version")?
                        .as_str()
                        .map(|version| version == raw_worker::RAW_WORKER_PROTOCOL_VERSION)
                })
                .unwrap_or(false);
        if looks {
            v2_mode = true;
            let shutdown = serde_json::from_slice::<raw_worker::WorkerRequestFrame>(&line)
                .map(|frame| matches!(frame, raw_worker::WorkerRequestFrame::Shutdown { .. }))
                .unwrap_or(false);
            let out = protocol_worker.handle_line(&ctx, &line);
            if response_bytes_are_error(&out) {
                saw_error = true;
            }
            let _ = stdout.write_all(&out);
            let _ = stdout.flush();
            if shutdown {
                break;
            }
            continue;
        }
        // v1 path (retained, unchanged).
        let frame: Value = match serde_json::from_slice(&line) {
            Ok(v) => v,
            Err(e) => {
                saw_error = true;
                let resp = WorkerResponseFrame::Error {
                    request_id: None,
                    error: crate::DomainError::new(
                        crate::DomainErrorKind::Validation,
                        format!("invalid frame: {e}"),
                    ),
                    trace: None,
                    compatibility: None,
                };
                let _ = writeln!(
                    stdout,
                    "{}",
                    serde_json::to_string(&resp).unwrap_or_default()
                );
                let _ = stdout.flush();
                continue;
            }
        };
        let req: WorkerRequestFrame = match serde_json::from_value(frame) {
            Ok(r) => r,
            Err(e) => {
                saw_error = true;
                let resp = WorkerResponseFrame::Error {
                    request_id: None,
                    error: crate::DomainError::new(
                        crate::DomainErrorKind::Validation,
                        format!("invalid worker request: {e}"),
                    ),
                    trace: None,
                    compatibility: None,
                };
                let _ = writeln!(
                    stdout,
                    "{}",
                    serde_json::to_string(&resp).unwrap_or_default()
                );
                let _ = stdout.flush();
                continue;
            }
        };
        let resp = worker.handle_frame(&ctx, &req);
        let encoded = serde_json::to_string(&resp).unwrap_or_default();
        if matches!(resp, WorkerResponseFrame::Error { .. })
            || response_bytes_are_error(encoded.as_bytes())
        {
            saw_error = true;
        }
        let _ = writeln!(stdout, "{encoded}");
        let _ = stdout.flush();
    }
    if saw_error { 1 } else { 0 }
}

fn response_bytes_are_error(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    match value.get("kind").and_then(|k| k.as_str()) {
        Some("error") => true,
        _ => {
            value
                .get("response")
                .and_then(|r| r.get("kind"))
                .and_then(|k| k.as_str())
                == Some("error")
        }
    }
}
