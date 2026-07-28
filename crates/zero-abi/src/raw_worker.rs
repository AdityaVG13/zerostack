//! Canonical ZeroStack private raw-worker v2 wire contract.
//!
//! Aggregate CodeMode owns JavaScript, scheduling, policy orchestration, refs,
//! journaling, and telemetry. A raw worker receives canonical typed operations
//! only. These types deliberately contain no planner, JavaScript, MCP, or
//! nested-CodeMode concept.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::digest::contract_digest_hex;

/// One protocol across FSZero, GraphZero, and TokenZero.
pub const RAW_WORKER_PROTOCOL_VERSION: &str = "zerostack.raw_worker.v2";

/// Default maximum encoded NDJSON frame, excluding the trailing newline.
pub const DEFAULT_MAX_FRAME_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    ReadOnly,
    ReversibleMutation,
    ApprovalRequiredMutation,
    Irreversible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    NotRequired,
    Required,
    Granted,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalMetadata {
    pub state: ApprovalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevertMetadata {
    pub supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_op: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotIdentity {
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefOwnership {
    pub engine: String,
    pub session_id: String,
    #[serde(default)]
    pub refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerTrace {
    pub runtime_id: String,
    pub cell_id: String,
    pub request_id: String,
    pub trace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub worker_revision: String,
    pub contract_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolLimits {
    pub max_frame_bytes: u64,
    pub max_output_bytes: u64,
    pub max_in_flight: u32,
    pub default_deadline_ms: u64,
}

impl Default for ProtocolLimits {
    fn default() -> Self {
        Self {
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES as u64,
            max_output_bytes: 65_536,
            max_in_flight: 1,
            default_deadline_ms: 30_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerCapabilities {
    pub cancellation: bool,
    pub deadlines: bool,
    pub approvals: bool,
    pub revert: bool,
    pub snapshots: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerBinding {
    pub engine: String,
    pub root: String,
    pub session_id: String,
    pub worker_revision: String,
    pub semantic_contract_version: String,
    pub semantic_contract_digest: String,
    pub operation_registry_digest: String,
    pub ref_scheme: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandshakeRequest {
    pub protocol_version: String,
    pub root: String,
    pub session_id: String,
    pub expected_engine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_worker_revision: Option<String>,
    pub expected_contract_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_registry_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandshakeAck {
    pub protocol_version: String,
    pub binding: WorkerBinding,
    pub capabilities: WorkerCapabilities,
    pub limits: ProtocolLimits,
    pub protocol_digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallRequest {
    pub request_id: String,
    pub op: String,
    pub args: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_unix_ms: Option<u64>,
    pub trace: WorkerTrace,
}

impl CallRequest {
    pub fn deadline_expired(&self, now_unix_ms: u64) -> bool {
        self.deadline_unix_ms
            .is_some_and(|deadline| deadline <= now_unix_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelRequest {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownRequest {
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerRequestFrame {
    Handshake { request: HandshakeRequest },
    Call { request: CallRequest },
    Cancel { request: CancelRequest },
    Shutdown { request: ShutdownRequest },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerResultMetadata {
    pub effect: EffectClass,
    pub approval: ApprovalMetadata,
    pub revert: RevertMetadata,
    pub ownership: RefOwnership,
    pub trace: WorkerTrace,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerResult {
    pub value: Value,
    pub metadata: WorkerResultMetadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerError {
    pub kind: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerResponseFrame {
    HandshakeAck {
        ack: HandshakeAck,
    },
    Result {
        request_id: String,
        result: WorkerResult,
    },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        error: WorkerError,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace: Option<WorkerTrace>,
    },
    CancelAck {
        request_id: String,
        cancelled: bool,
    },
    ShutdownAck,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameCodecError {
    Empty,
    TooLarge { actual: usize, maximum: usize },
    InvalidJson(String),
    InvalidContract(String),
}

impl FrameCodecError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Empty | Self::InvalidJson(_) => "invalid_frame",
            Self::TooLarge { .. } => "frame_too_large",
            Self::InvalidContract(_) => "contract_mismatch",
        }
    }
}

impl fmt::Display for FrameCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "raw-worker frame is empty"),
            Self::TooLarge { actual, maximum } => {
                write!(
                    f,
                    "raw-worker frame is {actual} bytes; maximum is {maximum}"
                )
            }
            Self::InvalidJson(message) | Self::InvalidContract(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for FrameCodecError {}

fn trim_frame(bytes: &[u8]) -> &[u8] {
    let line = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    line.strip_suffix(b"\r").unwrap_or(line)
}

pub fn decode_request_frame(
    bytes: &[u8],
    max_frame_bytes: usize,
) -> Result<WorkerRequestFrame, FrameCodecError> {
    let line = trim_frame(bytes);
    if line.is_empty() {
        return Err(FrameCodecError::Empty);
    }
    if line.len() > max_frame_bytes {
        return Err(FrameCodecError::TooLarge {
            actual: line.len(),
            maximum: max_frame_bytes,
        });
    }
    let frame: WorkerRequestFrame = serde_json::from_slice(line)
        .map_err(|error| FrameCodecError::InvalidJson(error.to_string()))?;
    validate_request_frame(&frame)?;
    Ok(frame)
}

pub fn decode_response_frame(
    bytes: &[u8],
    max_frame_bytes: usize,
) -> Result<WorkerResponseFrame, FrameCodecError> {
    let line = trim_frame(bytes);
    if line.is_empty() {
        return Err(FrameCodecError::Empty);
    }
    if line.len() > max_frame_bytes {
        return Err(FrameCodecError::TooLarge {
            actual: line.len(),
            maximum: max_frame_bytes,
        });
    }
    serde_json::from_slice(line).map_err(|error| FrameCodecError::InvalidJson(error.to_string()))
}

fn require_nonempty(field: &str, value: &str) -> Result<(), FrameCodecError> {
    if value.is_empty() {
        return Err(FrameCodecError::InvalidContract(format!(
            "{field} must be non-empty"
        )));
    }
    Ok(())
}

fn require_hex_digest(field: &str, value: &str) -> Result<(), FrameCodecError> {
    let is_hex = value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !is_hex {
        return Err(FrameCodecError::InvalidContract(format!(
            "{field} must be a 64-character lowercase hex digest"
        )));
    }
    Ok(())
}

fn validate_trace(trace: &WorkerTrace) -> Result<(), FrameCodecError> {
    require_nonempty("trace.runtime_id", &trace.runtime_id)?;
    require_nonempty("trace.cell_id", &trace.cell_id)?;
    require_nonempty("trace.request_id", &trace.request_id)?;
    require_nonempty("trace.trace_id", &trace.trace_id)?;
    require_nonempty("trace.worker_revision", &trace.worker_revision)?;
    require_hex_digest("trace.contract_digest", &trace.contract_digest)
}

/// Structural rules from raw-worker-v2.schema.json that serde alone cannot express.
pub fn validate_request_frame(frame: &WorkerRequestFrame) -> Result<(), FrameCodecError> {
    match frame {
        WorkerRequestFrame::Handshake { request } => {
            if request.protocol_version != RAW_WORKER_PROTOCOL_VERSION {
                return Err(handshake_field_mismatch(
                    "protocol_version",
                    RAW_WORKER_PROTOCOL_VERSION,
                    &request.protocol_version,
                ));
            }
            require_nonempty("handshake.root", &request.root)?;
            require_nonempty("handshake.session_id", &request.session_id)?;
            require_nonempty("handshake.expected_engine", &request.expected_engine)?;
            require_hex_digest(
                "handshake.expected_contract_digest",
                &request.expected_contract_digest,
            )?;
            if let Some(revision) = request.expected_worker_revision.as_deref() {
                require_nonempty("handshake.expected_worker_revision", revision)?;
            }
            if let Some(digest) = request.expected_registry_digest.as_deref() {
                require_hex_digest("handshake.expected_registry_digest", digest)?;
            }
            Ok(())
        }
        WorkerRequestFrame::Call { request } => {
            require_nonempty("call.request_id", &request.request_id)?;
            require_nonempty("call.op", &request.op)?;
            if request.deadline_unix_ms == Some(0) {
                return Err(FrameCodecError::InvalidContract(
                    "call.deadline_unix_ms must be at least 1".into(),
                ));
            }
            validate_trace(&request.trace)
        }
        WorkerRequestFrame::Cancel { request } => {
            require_nonempty("cancel.request_id", &request.request_id)
        }
        WorkerRequestFrame::Shutdown { request } => {
            require_nonempty("shutdown.reason", &request.reason)
        }
    }
}

pub fn encode_frame<T: Serialize>(
    frame: &T,
    max_frame_bytes: usize,
) -> Result<Vec<u8>, FrameCodecError> {
    let mut bytes = serde_json::to_vec(frame)
        .map_err(|error| FrameCodecError::InvalidJson(error.to_string()))?;
    if bytes.len() > max_frame_bytes {
        return Err(FrameCodecError::TooLarge {
            actual: bytes.len(),
            maximum: max_frame_bytes,
        });
    }
    bytes.push(b'\n');
    Ok(bytes)
}

fn handshake_field_mismatch(field: &str, expected: &str, actual: &str) -> FrameCodecError {
    FrameCodecError::InvalidContract(format!(
        "{field} mismatch: expected={expected} actual={actual}"
    ))
}

fn require_nonempty_handshake_ids(request: &HandshakeRequest) -> Result<(), FrameCodecError> {
    if request.root.is_empty() || request.session_id.is_empty() {
        return Err(FrameCodecError::InvalidContract(
            "handshake requires non-empty root and session_id".into(),
        ));
    }
    Ok(())
}

fn require_root_session_binding(
    request: &HandshakeRequest,
    binding: &WorkerBinding,
) -> Result<(), FrameCodecError> {
    if request.root != binding.root || request.session_id != binding.session_id {
        return Err(FrameCodecError::InvalidContract(
            "worker root/session binding mismatch".into(),
        ));
    }
    Ok(())
}

/// Optional client-supplied pin: absent means skip; present must equal binding.
fn check_optional_eq(
    field: &str,
    expected: Option<&str>,
    actual: &str,
) -> Result<(), FrameCodecError> {
    if let Some(expected) = expected {
        if expected != actual {
            return Err(handshake_field_mismatch(field, expected, actual));
        }
    }
    Ok(())
}

pub fn validate_handshake_request(
    request: &HandshakeRequest,
    binding: &WorkerBinding,
) -> Result<(), FrameCodecError> {
    // Field-specific messages are intentional; do not fold distinct fields into one bool.
    if request.protocol_version != RAW_WORKER_PROTOCOL_VERSION {
        return Err(handshake_field_mismatch(
            "protocol_version",
            RAW_WORKER_PROTOCOL_VERSION,
            &request.protocol_version,
        ));
    }
    require_nonempty_handshake_ids(request)?;
    require_root_session_binding(request, binding)?;
    for (field, expected, actual) in [
        (
            "engine",
            request.expected_engine.as_str(),
            binding.engine.as_str(),
        ),
        (
            "semantic_contract_digest",
            request.expected_contract_digest.as_str(),
            binding.semantic_contract_digest.as_str(),
        ),
    ] {
        if expected != actual {
            return Err(handshake_field_mismatch(field, expected, actual));
        }
    }
    check_optional_eq(
        "operation_registry_digest",
        request.expected_registry_digest.as_deref(),
        &binding.operation_registry_digest,
    )?;
    check_optional_eq(
        "worker_revision",
        request.expected_worker_revision.as_deref(),
        &binding.worker_revision,
    )?;
    Ok(())
}

/// Serialized field names of an exemplar value, in declaration order.
fn field_names<T: Serialize>(exemplar: &T) -> Vec<String> {
    match serde_json::to_value(exemplar) {
        Ok(Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// Exemplar with every optional field populated, so the manifest reflects the
/// full type-level surface rather than a hand-maintained list.
fn manifest_trace() -> WorkerTrace {
    WorkerTrace {
        runtime_id: String::new(),
        cell_id: String::new(),
        request_id: String::new(),
        trace_id: String::new(),
        parent_span_id: Some(String::new()),
        worker_revision: String::new(),
        contract_digest: String::new(),
    }
}

pub fn raw_worker_protocol_manifest() -> Value {
    let binding = WorkerBinding {
        engine: String::new(),
        root: String::new(),
        session_id: String::new(),
        worker_revision: String::new(),
        semantic_contract_version: String::new(),
        semantic_contract_digest: String::new(),
        operation_registry_digest: String::new(),
        ref_scheme: String::new(),
    };
    let call = CallRequest {
        request_id: String::new(),
        op: String::new(),
        args: Value::Null,
        deadline_unix_ms: Some(0),
        trace: manifest_trace(),
    };
    let result_metadata = WorkerResultMetadata {
        effect: EffectClass::ReadOnly,
        approval: ApprovalMetadata {
            state: ApprovalState::NotRequired,
            approval_id: Some(String::new()),
            policy: Some(String::new()),
        },
        revert: RevertMetadata {
            supported: false,
            journal_id: Some(String::new()),
            rollback_op: Some(String::new()),
        },
        ownership: RefOwnership {
            engine: String::new(),
            session_id: String::new(),
            refs: Vec::new(),
            snapshot: Some(SnapshotIdentity {
                kind: String::new(),
                id: String::new(),
                digest: Some(String::new()),
            }),
        },
        trace: manifest_trace(),
    };
    let handshake = HandshakeRequest {
        protocol_version: String::new(),
        root: String::new(),
        session_id: String::new(),
        expected_engine: String::new(),
        expected_worker_revision: Some(String::new()),
        expected_contract_digest: String::new(),
        expected_registry_digest: Some(String::new()),
    };
    let capabilities = WorkerCapabilities {
        cancellation: false,
        deadlines: false,
        approvals: false,
        revert: false,
        snapshots: false,
    };

    json!({
        "protocol_version": RAW_WORKER_PROTOCOL_VERSION,
        "framing": "bounded_ndjson",
        "default_max_frame_bytes": DEFAULT_MAX_FRAME_BYTES,
        "request_frames": ["handshake", "call", "cancel", "shutdown"],
        "response_frames": ["handshake_ack", "result", "error", "cancel_ack", "shutdown_ack"],
        "binding": field_names(&binding),
        "handshake_request": field_names(&handshake),
        "capabilities": field_names(&capabilities),
        "limits": field_names(&ProtocolLimits::default()),
        "call": field_names(&call),
        "trace": field_names(&manifest_trace()),
        "result_metadata": field_names(&result_metadata),
        "negative_space": ["planner", "javascript_runtime", "mcp_catalog", "nested_codemode"],
    })
}

pub fn raw_worker_protocol_digest_hex() -> String {
    contract_digest_hex(&raw_worker_protocol_manifest())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace() -> WorkerTrace {
        WorkerTrace {
            runtime_id: "runtime-1".into(),
            cell_id: "cell-1".into(),
            request_id: "request-1".into(),
            trace_id: "trace-1".into(),
            parent_span_id: None,
            worker_revision: "abc123".into(),
            contract_digest: "d".repeat(64),
        }
    }

    #[test]
    fn call_and_cancel_round_trip_through_bounded_ndjson() {
        let call = WorkerRequestFrame::Call {
            request: CallRequest {
                request_id: "request-1".into(),
                op: "read".into(),
                args: json!({"path": "README.md"}),
                deadline_unix_ms: Some(100),
                trace: trace(),
            },
        };
        let encoded = encode_frame(&call, DEFAULT_MAX_FRAME_BYTES).unwrap();
        assert_eq!(
            decode_request_frame(&encoded, DEFAULT_MAX_FRAME_BYTES).unwrap(),
            call
        );

        let cancel = WorkerRequestFrame::Cancel {
            request: CancelRequest {
                request_id: "request-1".into(),
                reason: Some("cell terminated".into()),
            },
        };
        let encoded = encode_frame(&cancel, DEFAULT_MAX_FRAME_BYTES).unwrap();
        assert_eq!(
            decode_request_frame(&encoded, DEFAULT_MAX_FRAME_BYTES).unwrap(),
            cancel
        );
    }

    fn call_frame_bytes(op: &str, deadline: Option<u64>) -> Vec<u8> {
        encode_frame(
            &WorkerRequestFrame::Call {
                request: CallRequest {
                    request_id: "request-1".into(),
                    op: op.into(),
                    args: json!({}),
                    deadline_unix_ms: deadline,
                    trace: trace(),
                },
            },
            DEFAULT_MAX_FRAME_BYTES,
        )
        .unwrap()
    }

    #[test]
    fn decode_request_frame_rejects_schema_violations() {
        for bytes in [
            call_frame_bytes("", None),
            call_frame_bytes("read", Some(0)),
        ] {
            assert!(matches!(
                decode_request_frame(&bytes, DEFAULT_MAX_FRAME_BYTES),
                Err(FrameCodecError::InvalidContract(_))
            ));
        }

        let missing_args = br#"{"kind":"call","request":{"request_id":"r","op":"read","trace":{"runtime_id":"r","cell_id":"c","request_id":"r","trace_id":"t","worker_revision":"w","contract_digest":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}}}"#;
        assert!(matches!(
            decode_request_frame(missing_args, DEFAULT_MAX_FRAME_BYTES),
            Err(FrameCodecError::InvalidJson(_))
        ));

        let bad_digest = encode_frame(
            &WorkerRequestFrame::Handshake {
                request: HandshakeRequest {
                    protocol_version: RAW_WORKER_PROTOCOL_VERSION.into(),
                    root: "/repo".into(),
                    session_id: "session-1".into(),
                    expected_engine: "fszero".into(),
                    expected_worker_revision: None,
                    expected_contract_digest: "NOTHEX".into(),
                    expected_registry_digest: None,
                },
            },
            DEFAULT_MAX_FRAME_BYTES,
        )
        .unwrap();
        assert_eq!(
            decode_request_frame(&bad_digest, DEFAULT_MAX_FRAME_BYTES)
                .unwrap_err()
                .kind(),
            "contract_mismatch"
        );

        let empty_reason = encode_frame(
            &WorkerRequestFrame::Shutdown {
                request: ShutdownRequest {
                    reason: String::new(),
                },
            },
            DEFAULT_MAX_FRAME_BYTES,
        )
        .unwrap();
        assert!(matches!(
            decode_request_frame(&empty_reason, DEFAULT_MAX_FRAME_BYTES),
            Err(FrameCodecError::InvalidContract(_))
        ));
    }

    #[test]
    fn protocol_version_mismatch_reports_expected_canonical_version() {
        let request = HandshakeRequest {
            protocol_version: "zerostack.raw_worker.v1".into(),
            root: "/repo".into(),
            session_id: "session-1".into(),
            expected_engine: "fszero".into(),
            expected_worker_revision: None,
            expected_contract_digest: "d".repeat(64),
            expected_registry_digest: None,
        };
        let message = validate_request_frame(&WorkerRequestFrame::Handshake { request })
            .unwrap_err()
            .to_string();
        assert_eq!(
            message,
            "protocol_version mismatch: expected=zerostack.raw_worker.v2 actual=zerostack.raw_worker.v1"
        );
    }

    #[test]
    fn frame_size_boundary_is_inclusive_at_max() {
        let at_max = vec![b'x'; DEFAULT_MAX_FRAME_BYTES];
        assert!(matches!(
            decode_request_frame(&at_max, DEFAULT_MAX_FRAME_BYTES),
            Err(FrameCodecError::InvalidJson(_))
        ));

        let over_max = vec![b'x'; DEFAULT_MAX_FRAME_BYTES + 1];
        assert_eq!(
            decode_request_frame(&over_max, DEFAULT_MAX_FRAME_BYTES).unwrap_err(),
            FrameCodecError::TooLarge {
                actual: DEFAULT_MAX_FRAME_BYTES + 1,
                maximum: DEFAULT_MAX_FRAME_BYTES,
            }
        );
        assert!(matches!(
            decode_response_frame(&over_max, DEFAULT_MAX_FRAME_BYTES),
            Err(FrameCodecError::TooLarge { .. })
        ));
    }

    #[test]
    fn decode_response_frame_round_trips_and_is_size_bounded() {
        let ack = WorkerResponseFrame::CancelAck {
            request_id: "request-1".into(),
            cancelled: true,
        };
        let encoded = encode_frame(&ack, DEFAULT_MAX_FRAME_BYTES).unwrap();
        assert_eq!(
            decode_response_frame(&encoded, DEFAULT_MAX_FRAME_BYTES).unwrap(),
            ack
        );
        assert!(matches!(
            decode_response_frame(&encoded, 8),
            Err(FrameCodecError::TooLarge { .. })
        ));
        assert!(matches!(
            decode_response_frame(b"\n", DEFAULT_MAX_FRAME_BYTES),
            Err(FrameCodecError::Empty)
        ));
    }

    #[test]
    fn protocol_manifest_covers_type_level_binding_surface() {
        let manifest = raw_worker_protocol_manifest();
        let binding = manifest["binding"].as_array().unwrap();
        for field in ["semantic_contract_version", "ref_scheme"] {
            assert!(binding.iter().any(|value| value == field), "{field}");
        }
        assert!(manifest["trace"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "parent_span_id"));
    }

    #[test]
    fn oversized_and_unknown_frames_fail_closed() {
        let oversized = vec![b'x'; 33];
        assert!(matches!(
            decode_request_frame(&oversized, 32),
            Err(FrameCodecError::TooLarge { .. })
        ));
        let unknown = br#"{"kind":"call","request":{"request_id":"r","op":"x","args":{},"trace":{"runtime_id":"r","cell_id":"c","request_id":"r","trace_id":"t","worker_revision":"w","contract_digest":"d"}},"ambient_node":true}"#;
        assert!(matches!(
            decode_request_frame(unknown, DEFAULT_MAX_FRAME_BYTES),
            Err(FrameCodecError::InvalidJson(_))
        ));
    }

    #[test]
    fn handshake_rejects_skew_and_wrong_binding() {
        let binding = WorkerBinding {
            engine: "fszero".into(),
            root: "/repo".into(),
            session_id: "session-1".into(),
            worker_revision: "abc123".into(),
            semantic_contract_version: "1".into(),
            semantic_contract_digest: "a".repeat(64),
            operation_registry_digest: "b".repeat(64),
            ref_scheme: "fz".into(),
        };
        let mut request = HandshakeRequest {
            protocol_version: RAW_WORKER_PROTOCOL_VERSION.into(),
            root: binding.root.clone(),
            session_id: binding.session_id.clone(),
            expected_engine: binding.engine.clone(),
            expected_worker_revision: Some(binding.worker_revision.clone()),
            expected_contract_digest: binding.semantic_contract_digest.clone(),
            expected_registry_digest: Some(binding.operation_registry_digest.clone()),
        };
        validate_handshake_request(&request, &binding).unwrap();
        request.session_id = "other-session".into();
        assert_eq!(
            validate_handshake_request(&request, &binding)
                .unwrap_err()
                .kind(),
            "contract_mismatch"
        );
    }

    #[test]
    fn deadline_and_protocol_digest_are_deterministic() {
        let call = CallRequest {
            request_id: "request-1".into(),
            op: "read".into(),
            args: Value::Null,
            deadline_unix_ms: Some(100),
            trace: trace(),
        };
        assert!(!call.deadline_expired(99));
        assert!(call.deadline_expired(100));
        let digest = raw_worker_protocol_digest_hex();
        assert_eq!(digest.len(), 64);
        assert_eq!(
            digest,
            "5c887d5b3443ec572b153cbd635b205d3e68f54308a3815d5878b59842e9fd38"
        );
        assert_eq!(digest, raw_worker_protocol_digest_hex());
    }
}
