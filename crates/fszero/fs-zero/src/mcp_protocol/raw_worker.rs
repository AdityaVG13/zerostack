//! Production private raw-worker stdio loop (fszero-ncib.4).
//!
//! Internal mode of the selected surface artifact: NDJSON frames over stdin/
//! stdout. Invokes `PrivateRawWorker` only — never plans, parses JS, or starts
//! a sandbox. Enabled with `--raw-worker` or `FSZERO_PRIVATE_WORKER=1`.

use crate::core::runtime_metrics;
use crate::core::{
    DomainError, DomainResult, FSZeroSession, HandshakeRequest, Ownership, PrivateRawWorker,
    SelectedSurface, WorkerRequestFrame, WorkerResponseFrame, WorkerTrace, contract_digest_hex,
};
use crate::core::{RAW_WORKER_MAX_FRAME_BYTES, RAW_WORKER_PROTOCOL_VERSION, RawWorker};
use serde_json::Value;
use std::io::{self, BufRead, Write};
use std::path::Path;

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Whether argv/env selects the private raw-worker mode.
pub fn raw_worker_requested(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--raw-worker" || a == "--mode=raw-worker")
        || env_truthy("FSZERO_PRIVATE_WORKER")
}

fn selected_surface() -> SelectedSurface {
    match crate::packaging::baked_package_surface() {
        Some(crate::packaging::PackageSurface::Codemode) => SelectedSurface::Codemode,
        Some(crate::packaging::PackageSurface::Mcp) | None => SelectedSurface::Mcp,
    }
}

/// Run NDJSON private-worker protocol until stdin EOF.
pub fn run_raw_worker_stdio(args: &[String]) -> Result<(), String> {
    let root = crate::mcp_rpc::resolve_cli_root(args);
    if std::env::var("ZEROSTACK_RAW_WORKER_PROTOCOL")
        .is_ok_and(|value| value == RAW_WORKER_PROTOCOL_VERSION)
    {
        return serve_raw_worker(&root);
    }
    let surface = selected_surface();
    let mut session = FSZeroSession::with_root(&root);
    let outer =
        std::env::var("FSZERO_PLANNER_OWNER").is_ok_and(|v| v.eq_ignore_ascii_case("outer_router"));
    let mut worker = if outer {
        PrivateRawWorker::for_outer_router(surface)
    } else {
        PrivateRawWorker::for_client_native(surface)
    };

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut input = stdin.lock();
    loop {
        let line = match read_bounded_frame(&mut input, RAW_WORKER_MAX_FRAME_BYTES)
            .map_err(|e| format!("raw-worker stdin: {e}"))?
        {
            BoundedFrame::Eof => break,
            BoundedFrame::TooLarge => {
                let out = WorkerResponseFrame::Error {
                    request_id: None,
                    error: DomainError::invalid_argument(
                        "frame_too_large: inbound frame exceeds 1 MiB",
                    ),
                    trace: None,
                    compatibility: None,
                };
                writeln!(
                    stdout,
                    "{}",
                    serde_json::to_string(&out).map_err(|e| e.to_string())?
                )
                .map_err(|e| format!("raw-worker stdout: {e}"))?;
                stdout
                    .flush()
                    .map_err(|e| format!("raw-worker flush: {e}"))?;
                continue;
            }
            BoundedFrame::Line(line) => line,
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let frame: Value = serde_json::from_slice(&line)
            .map_err(|e| format!("raw-worker invalid JSON frame: {e}"))?;
        let out = match worker.handle_json(&mut session, &frame) {
            Ok(r) => r,
            Err(e) => WorkerResponseFrame::Error {
                request_id: None,
                error: e,
                trace: None,
                compatibility: Some(serde_json::json!({
                    "semantic_contract_digest": contract_digest_hex(), "hint": "send handshake frame with matching semantic_contract_digest first",
                })),
            },
        };
        let encoded = serde_json::to_string(&out).map_err(|e| e.to_string())?;
        runtime_metrics::record_serialization(encoded.len());
        writeln!(stdout, "{encoded}").map_err(|e| format!("raw-worker stdout: {e}"))?;
        stdout
            .flush()
            .map_err(|e| format!("raw-worker flush: {e}"))?;
    }
    Ok(())
}

/// Run the canonical raw-worker protocol regardless of compatibility env.
///
/// The canonical `fszero-worker` package calls this directly so its
/// no-default-feature binary cannot enter the legacy or CodeMode surfaces.
pub fn run_raw_worker_protocol_stdio(args: &[String]) -> Result<(), String> {
    let root = crate::mcp_rpc::resolve_cli_root(args);
    serve_raw_worker(&root)
}

fn serve_raw_worker(root: &Path) -> Result<(), String> {
    let session_id =
        std::env::var("ZEROSTACK_SESSION_ID").unwrap_or_else(|_| "fszero-raw-worker".into());
    let mut worker = RawWorker::new(root.to_string_lossy().into_owned(), session_id);
    let mut session = FSZeroSession::with_root(root);
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut stdout = io::stdout();
    loop {
        let line = match read_bounded_frame(&mut input, RAW_WORKER_MAX_FRAME_BYTES)
            .map_err(|e| format!("raw-worker v2 stdin: {e}"))?
        {
            BoundedFrame::Eof => return Ok(()),
            BoundedFrame::TooLarge => vec![b'x'; RAW_WORKER_MAX_FRAME_BYTES + 1],
            BoundedFrame::Line(line) => line,
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let encoded = worker.handle_line(&mut session, &line);
        let shutdown = v2_response_requests_termination(&encoded);
        runtime_metrics::record_serialization(encoded.len());
        stdout
            .write_all(&encoded)
            .map_err(|e| format!("raw-worker v2 stdout: {e}"))?;
        stdout
            .flush()
            .map_err(|e| format!("raw-worker v2 flush: {e}"))?;
        if shutdown {
            return Ok(());
        }
    }
}

fn v2_response_requests_termination(encoded: &[u8]) -> bool {
    matches!(
        crate::core::raw_worker_protocol::decode_response_frame(
            encoded,
            RAW_WORKER_MAX_FRAME_BYTES,
        ),
        Ok(crate::core::raw_worker_protocol::WorkerResponseFrame::ShutdownAck)
    )
}

enum BoundedFrame {
    Eof,
    Line(Vec<u8>),
    TooLarge,
}

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

/// Single-shot production helper: handshake + one op (install smoke / composition).
pub fn raw_worker_call_once(
    root: &Path,
    surface: SelectedSurface,
    op: &str,
    args: &Value,
) -> Result<(DomainResult, WorkerTrace), DomainError> {
    let mut session = FSZeroSession::with_root(root);
    let mut worker = PrivateRawWorker::for_client_native(surface);
    worker.handshake(&HandshakeRequest {
        semantic_contract_digest: Some(contract_digest_hex()),
        planner_owner: Some(Ownership::Client),
        compression_owner: Some(Ownership::Client),
        expect_surface: Some(surface),
        ..Default::default()
    })?;
    let (result, trace, _tele) = worker.call(&mut session, op, args)?;
    Ok((result, trace))
}

/// Structural: frame types are production API surface.
pub fn supports_handshake_and_call_frames() -> bool {
    let _ = std::mem::size_of::<WorkerRequestFrame>();
    true
}
