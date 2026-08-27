//! Additive canonical raw-worker v2 adapter for FSZero.
//!
//! The v1 worker remains available for compatibility. This path is selected by
//! the aggregate sidecar and owns no planner, JavaScript runtime, or MCP catalog.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::dispatcher::dispatch_raw_worker;
use super::operation_abi::OPERATION_ABI_VERSION;
use super::raw_worker_protocol::*;
use super::session::FSZeroSession;
use super::surface_handshake::contract_digest_hex;

pub fn resolve_worker_revision(env_value: Option<&str>) -> String {
    env_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

fn worker_revision() -> String {
    resolve_worker_revision(std::env::var("ZEROSTACK_WORKER_REVISION").ok().as_deref())
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn finish_timed_call(
    mut response: WorkerResponseFrame,
    request: &CallRequest,
    started: Instant,
) -> WorkerResponseFrame {
    if !request
        .telemetry_request
        .as_ref()
        .is_some_and(|telemetry| telemetry.engine_stage_timeline)
    {
        return response;
    }
    let duration_ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
    let duration_ns = duration_ns.max(1);
    let timeline = zero_abi::EngineStageTimeline {
        total_ns: duration_ns,
        spans: vec![zero_abi::EngineStageSpan {
            stage: "fszero.raw_worker_call".into(),
            start_ns: 0,
            duration_ns,
        }],
    };
    debug_assert!(zero_abi::validate_engine_stage_timeline(&timeline).is_ok());
    match &mut response {
        WorkerResponseFrame::Result {
            engine_timeline, ..
        }
        | WorkerResponseFrame::Error {
            engine_timeline, ..
        } => *engine_timeline = Some(timeline),
        _ => {}
    }
    response
}

fn forbidden(op: &str) -> bool {
    matches!(
        op,
        "execute_code"
            | "fz_execute_code"
            | "codemode_search"
            | "fz_codemode_search"
            | "codemode_describe"
            | "fz_codemode_describe"
            | "tools/call"
            | "tools/list"
            | "fszero.exec"
    )
}

fn domain_error_kind(class: &str) -> String {
    match class {
        "invalid_argument" => "validation".into(),
        "permission_denied" | "incompatible_contract" => "policy".into(),
        "cancelled" => "cancelled".into(),
        "deadline_exceeded" => "deadline_exceeded".into(),
        "busy" => "busy".into(),
        other => other.into(),
    }
}

/// Boundary conformance for emitted refs (call-issues-0729 item 8, fs side):
/// an `fz://blob/` ref must carry exactly 64 lowercase hex characters.
pub fn is_conformant_blob_ref(reference: &str) -> bool {
    let Some(hash) = reference.strip_prefix("fz://blob/") else {
        return true;
    };
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Replace internal recovery labels (`read`, `last_cert`) with their canonical
/// public refs stored at `<label>/ref`. Unknown labels stay internal.
fn normalize_recovery_refs(session: &FSZeroSession, refs: &mut Vec<String>) {
    let mut normalized = Vec::with_capacity(refs.len());
    for reference in refs.drain(..) {
        let candidate = if reference.starts_with("fz://") {
            Some(reference)
        } else if reference.contains("://") {
            None
        } else {
            session
                .expand(&format!("{reference}/ref"))
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map(|value| value.trim().to_string())
                .filter(|value| value.starts_with("fz://"))
        };
        if let Some(candidate) = candidate
            && !normalized.contains(&candidate)
        {
            normalized.push(candidate);
        }
    }
    *refs = normalized;
}

/// Drop refs that are not FSZero-owned, byte-conformant, and expandable, so a
/// truncated, foreign, or unrecoverable ref never leaves the ownership boundary.
fn retain_valid_refs(session: &FSZeroSession, refs: &mut Vec<String>) -> Option<String> {
    let mut rejected = None;
    refs.retain(|reference| {
        let valid = reference.starts_with("fz://")
            && is_conformant_blob_ref(reference)
            && session.expand(reference).is_some();
        if !valid && rejected.is_none() {
            rejected = Some(reference.clone());
        }
        valid
    });
    rejected
}

fn collect_fszero_refs(value: &Value, refs: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            for token in text.split_whitespace() {
                if let Some(start) = token.find("fz://") {
                    let candidate = token[start..]
                        .trim_end_matches(['"', '\'', ',', ';', ')', '}', ']'])
                        .to_string();
                    if !refs.contains(&candidate) {
                        refs.push(candidate);
                    }
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_fszero_refs(item, refs);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_fszero_refs(item, refs);
            }
        }
        _ => {}
    }
}

#[derive(Debug)]
pub struct RawWorker {
    binding: WorkerBinding,
    capabilities: WorkerCapabilities,
    limits: ProtocolLimits,
    protocol_digest: String,
    handshook: bool,
}

impl RawWorker {
    pub fn new(root: impl Into<String>, session_id: impl Into<String>) -> Self {
        let digest = contract_digest_hex();
        Self {
            binding: WorkerBinding {
                engine: EngineIdentity::FsZero,
                root: root.into(),
                session_id: session_id.into(),
                worker_revision: worker_revision(),
                semantic_contract_version: OPERATION_ABI_VERSION.into(),
                semantic_contract_digest: digest.clone(),
                operation_registry_digest: digest,
                ref_scheme: "fz://".into(),
            },
            capabilities: WorkerCapabilities {
                // The synchronous worker cannot consume a cancel frame while a
                // call is active. The aggregate sidecar provides real active
                // cancellation by terminating the worker process.
                cancellation: false,
                deadlines: true,
                approvals: false,
                revert: true,
                snapshots: true,
            },
            limits: ProtocolLimits::default(),
            protocol_digest: raw_worker_protocol_digest_hex(),
            handshook: false,
        }
    }

    pub fn binding(&self) -> &WorkerBinding {
        &self.binding
    }

    fn error(
        request_id: Option<String>,
        kind: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
        trace: Option<WorkerTrace>,
    ) -> WorkerResponseFrame {
        WorkerResponseFrame::Error {
            request_id,
            error: WorkerError {
                kind: kind.into(),
                message: message.into(),
                retryable,
                details: None,
            },
            trace,
            engine_timeline: None,
            worker_token_accounting: None,
        }
    }

    fn bound_trace(&self, request: &CallRequest) -> WorkerTrace {
        WorkerTrace {
            runtime_id: request.trace.runtime_id.clone(),
            cell_id: request.trace.cell_id.clone(),
            request_id: request.request_id.clone(),
            trace_id: request.trace.trace_id.clone(),
            parent_span_id: request.trace.parent_span_id.clone(),
            worker_revision: self.binding.worker_revision.clone(),
            contract_digest: self.binding.semantic_contract_digest.clone(),
        }
    }

    pub fn handle_frame(
        &mut self,
        session: &mut FSZeroSession,
        frame: &WorkerRequestFrame,
    ) -> WorkerResponseFrame {
        match frame {
            WorkerRequestFrame::Handshake { request } => {
                match validate_handshake_request(request, &self.binding) {
                    Ok(()) => {
                        self.handshook = true;
                        WorkerResponseFrame::HandshakeAck {
                            ack: HandshakeAck {
                                protocol_version: RAW_WORKER_PROTOCOL_VERSION.into(),
                                binding: self.binding.clone(),
                                capabilities: self.capabilities.clone(),
                                limits: self.limits.clone(),
                                protocol_digest: self.protocol_digest.clone(),
                            },
                        }
                    }
                    Err(error) => Self::error(None, error.kind(), error.to_string(), false, None),
                }
            }
            WorkerRequestFrame::Call { request } => {
                let started = Instant::now();
                if !self.handshook {
                    return finish_timed_call(
                        Self::error(
                            Some(request.request_id.clone()),
                            "handshake_required",
                            "v2 call requires a completed handshake binding first",
                            false,
                            None,
                        ),
                        request,
                        started,
                    );
                }
                if request.deadline_expired(now_unix_ms()) {
                    return finish_timed_call(
                        Self::error(
                            Some(request.request_id.clone()),
                            "deadline_exceeded",
                            "call deadline_unix_ms expired before dispatch",
                            false,
                            Some(self.bound_trace(request)),
                        ),
                        request,
                        started,
                    );
                }
                if request.trace.request_id != request.request_id
                    || request.trace.worker_revision != self.binding.worker_revision
                    || request.trace.contract_digest != self.binding.semantic_contract_digest
                {
                    return finish_timed_call(
                        Self::error(
                            Some(request.request_id.clone()),
                            "trace_binding_mismatch",
                            "call trace does not match request/worker binding",
                            false,
                            Some(request.trace.clone()),
                        ),
                        request,
                        started,
                    );
                }
                if forbidden(&request.op) {
                    return finish_timed_call(
                        Self::error(
                            Some(request.request_id.clone()),
                            "forbidden_op",
                            "raw worker v2 refuses planner/JavaScript/MCP operations",
                            false,
                            Some(self.bound_trace(request)),
                        ),
                        request,
                        started,
                    );
                }

                let _ = session.take_mutation_outcome();
                let outcome = dispatch_raw_worker(session, &request.op, &request.args);
                let trace = self.bound_trace(request);
                let inline_evidence = outcome.inline_evidence;
                let mut result = outcome.result;
                if !result.ok {
                    let error = result.error.unwrap_or_else(|| {
                        super::operation_abi::DomainError::internal(format!(
                            "raw worker operation '{}' failed without typed error",
                            request.op
                        ))
                    });
                    let details = session
                        .take_mutation_outcome()
                        .and_then(|outcome| serde_json::to_value(outcome).ok());
                    return finish_timed_call(
                        WorkerResponseFrame::Error {
                            request_id: Some(request.request_id.clone()),
                            error: WorkerError {
                                kind: domain_error_kind(&error.class),
                                message: error.message,
                                retryable: error.retryable,
                                details,
                            },
                            trace: Some(trace),
                            engine_timeline: None,
                            worker_token_accounting: None,
                        },
                        request,
                        started,
                    );
                }
                let mutated = result.mutated;
                let mut refs = result.refs.clone();
                normalize_recovery_refs(session, &mut refs);
                if let Some(value) = &result.value {
                    collect_fszero_refs(value, &mut refs);
                }
                if let Some(reference) = request.args.get("ref").and_then(Value::as_str)
                    && reference.starts_with("fz://")
                    && !refs.iter().any(|value| value == reference)
                {
                    refs.push(reference.to_string());
                }
                if let Some(rejected) = retain_valid_refs(session, &mut refs) {
                    return finish_timed_call(
                        Self::error(
                            Some(request.request_id.clone()),
                            "ref_conformance",
                            format!(
                                "refusing to emit non-conformant ref {rejected:?}: ownership refs must be expandable fz:// refs and blob hashes must be 64 lowercase hex characters"
                            ),
                            false,
                            Some(trace),
                        ),
                        request,
                        started,
                    );
                }
                if let Some(key) = outcome
                    .recovery_key
                    .as_deref()
                    .or_else(|| result.refs.first().map(String::as_str))
                    && let Some(bytes) = session.expand(key)
                {
                    let portable_ref = refs
                        .iter()
                        .find(|value| value.starts_with("fz://"))
                        .cloned();
                    let payload = match String::from_utf8(bytes) {
                        Ok(text) => match portable_ref.as_ref() {
                            Some(reference) => serde_json::json!({
                                "ref": reference,
                                "payload_utf8": text,
                            }),
                            None => serde_json::json!({"payload_utf8": text}),
                        },
                        Err(error) => {
                            let bytes = error.into_bytes();
                            let hex: String =
                                bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                            match portable_ref.as_ref() {
                                Some(reference) => serde_json::json!({
                                    "ref": reference,
                                    "payload_hex": hex,
                                    "bytes_len": bytes.len(),
                                }),
                                None => serde_json::json!({
                                    "payload_hex": hex,
                                    "bytes_len": bytes.len(),
                                }),
                            }
                        }
                    };
                    result.value = Some(match result.value.take() {
                        Some(Value::Object(mut map)) => {
                            if let Value::Object(payload) = payload {
                                map.extend(payload);
                            }
                            Value::Object(map)
                        }
                        Some(prior) => serde_json::json!({ "prior": prior, "recovered": payload }),
                        None => payload,
                    });
                }
                result.refs = refs.clone();
                debug_assert!(
                    refs.iter().all(|reference| {
                        reference.starts_with("fz://")
                            && is_conformant_blob_ref(reference)
                            && session.expand(reference).is_some()
                    }),
                    "emitted refs must be conformant: {refs:?}"
                );
                let journal_id = if mutated {
                    refs.iter()
                        .find(|value| value.contains("journal") || value.contains("undo"))
                        .cloned()
                } else {
                    None
                };
                let mut value = serde_json::to_value(&result).unwrap_or(Value::Null);
                if let (Some(evidence), Value::Object(map)) = (inline_evidence.as_ref(), &mut value)
                {
                    map.insert("evidence".into(), serde_json::json!(evidence));
                }
                let response = WorkerResponseFrame::Result {
                    request_id: request.request_id.clone(),
                    result: WorkerResult {
                        value,
                        metadata: WorkerResultMetadata {
                            effect: if mutated {
                                EffectClass::ReversibleMutation
                            } else {
                                EffectClass::ReadOnly
                            },
                            approval: ApprovalMetadata {
                                state: ApprovalState::NotRequired,
                                approval_id: None,
                                policy: None,
                            },
                            revert: RevertMetadata {
                                supported: mutated,
                                journal_id,
                                rollback_op: mutated.then(|| "undo".into()),
                            },
                            ownership: RefOwnership {
                                engine: EngineIdentity::FsZero,
                                session_id: self.binding.session_id.clone(),
                                refs,
                                snapshot: None,
                            },
                            trace,
                        },
                    },
                    engine_timeline: None,
                    worker_token_accounting: None,
                };
                finish_timed_call(response, request, started)
            }
            WorkerRequestFrame::Cancel { request } => WorkerResponseFrame::CancelAck {
                request_id: request.request_id.clone(),
                cancelled: false,
            },
            WorkerRequestFrame::Shutdown { .. } => {
                self.handshook = false;
                WorkerResponseFrame::ShutdownAck
            }
        }
    }

    pub fn handle_line(&mut self, session: &mut FSZeroSession, bytes: &[u8]) -> Vec<u8> {
        let response = match decode_request_frame(bytes, DEFAULT_MAX_FRAME_BYTES) {
            Ok(frame) => self.handle_frame(session, &frame),
            Err(error) => Self::error(None, error.kind(), error.to_string(), false, None),
        };
        match encode_frame(&response, DEFAULT_MAX_FRAME_BYTES) {
            Ok(encoded) => encoded,
            Err(error) => {
                let fallback = Self::error(None, error.kind(), error.to_string(), false, None);
                encode_frame(&fallback, DEFAULT_MAX_FRAME_BYTES).unwrap_or_else(|_| {
                    b"{\"kind\":\"error\",\"error\":{\"kind\":\"frame_too_large\",\"message\":\"response exceeds maximum frame bytes\",\"retryable\":false}}\n".to_vec()
                })
            }
        }
    }
}
