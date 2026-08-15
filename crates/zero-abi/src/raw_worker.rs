//! Canonical ZeroStack private raw-worker v2 wire contract.
//!
//! Aggregate CodeMode owns JavaScript, scheduling, policy orchestration, refs,
//! journaling, and telemetry. A raw worker receives canonical typed operations
//! only. These types deliberately contain no planner, JavaScript, MCP, or
//! nested-CodeMode concept.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use crate::{
    assembly::{
        ASSEMBLY_ABI_CONTRACT_VERSION, ASSEMBLY_MANIFEST_SCHEMA_VERSION,
        assembly_abi_contract_digest_v1,
    },
    digest::contract_digest_hex,
    robust_snap::{
        ROBUST_SNAP_CONTRACT_VERSION, ROBUST_SNAP_MODEL_VERSION, robust_snap_contract_digest_v1,
    },
};

/// One protocol across FSZero, GraphZero, and TokenZero.
pub const RAW_WORKER_PROTOCOL_VERSION: &str = "zerostack.raw_worker.v2";

/// Default maximum encoded NDJSON frame, excluding the trailing newline.
pub const DEFAULT_MAX_FRAME_BYTES: usize = 1_048_576;
pub const ENGINE_TIMELINE_MAX_SPANS_V1: usize = 128;
pub const TIMELINE_CLOSURE_TOLERANCE_NS_V1: u64 = 250_000;

/// Closed identity set shared by all raw-worker protocol frames.
/// Canonical writes use stable names. Aliases are deliberate legacy reads;
/// every other spelling fails closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EngineIdentity {
    #[serde(rename = "fszero", alias = "fs_zero", alias = "fs")]
    FsZero,
    #[serde(rename = "graphzero", alias = "graph_zero", alias = "graph")]
    GraphZero,
    #[serde(rename = "tokenzero", alias = "token_zero", alias = "token")]
    TokenZero,
}

impl EngineIdentity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FsZero => "fszero",
            Self::GraphZero => "graphzero",
            Self::TokenZero => "tokenzero",
        }
    }
}

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
    pub engine: EngineIdentity,
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
    pub engine: EngineIdentity,
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
    pub expected_engine: EngineIdentity,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryRequestV1 {
    pub engine_stage_timeline: bool,
    pub worker_token_accounting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerTokenCountKind {
    Exact,
    ConservativeUpperBound,
    Estimate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerTokenAccountingV1 {
    pub tokenizer_id: String,
    /// Version digest of the tokenizer that produced this accounting
    /// (64 lowercase hex). Present for measured accounting; `None` when the
    /// adapter cannot bind a tokenizer version (e.g. estimator identities).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokenizer_version_digest: Option<String>,
    pub count_kind: WorkerTokenCountKind,
    pub raw_tokens: u64,
    pub visible_tokens: u64,
    pub recovery_tokens: u64,
    pub billed_tokens: u64,
    pub cached_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_ref_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineStageSpanV1 {
    pub stage: String,
    pub start_ns: u64,
    pub duration_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineStageTimelineV1 {
    pub total_ns: u64,
    pub spans: Vec<EngineStageSpanV1>,
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
    /// Additive v2 field: absent remains valid for non-approval operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_grant: Option<ApprovalGrant>,
    /// Default-disabled transport telemetry request. Domain arguments never
    /// carry this bit, so disabled request bytes remain byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_request: Option<TelemetryRequestV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalGrant {
    pub grant_id: String,
    pub engine: EngineIdentity,
    pub root: String,
    pub session_id: String,
    pub request_id: String,
    pub operation: String,
    pub effect: EffectClass,
    pub authority_digest: String,
    pub policy_digest: String,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalGrantRejection {
    Missing,
    Malformed,
    BindingMismatch,
    WrongEffect,
    Expired,
    Replayed,
}

impl CallRequest {
    /// Validate and consume an approval grant immediately before its action.
    pub fn validate_approval_grant(
        &self,
        expected_engine: EngineIdentity,
        expected_root: &str,
        expected_session_id: &str,
        expected_effect: EffectClass,
        now_unix_ms: u64,
        consumed_grants: &mut std::collections::BTreeSet<String>,
    ) -> Result<(), ApprovalGrantRejection> {
        let required = expected_effect == EffectClass::ApprovalRequiredMutation;
        let Some(grant) = &self.approval_grant else {
            return if required {
                Err(ApprovalGrantRejection::Missing)
            } else {
                Ok(())
            };
        };
        let lower_hex = |v: &str| {
            v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if grant.grant_id.is_empty()
            || grant.root.is_empty()
            || grant.session_id.is_empty()
            || grant.request_id.is_empty()
            || grant.operation.is_empty()
            || !lower_hex(&grant.authority_digest)
            || !lower_hex(&grant.policy_digest)
            || grant.issued_at_unix_ms >= grant.expires_at_unix_ms
        {
            return Err(ApprovalGrantRejection::Malformed);
        }
        if grant.engine != expected_engine
            || grant.root != expected_root
            || grant.session_id != expected_session_id
            || grant.request_id != self.request_id
            || grant.operation != self.op
        {
            return Err(ApprovalGrantRejection::BindingMismatch);
        }
        if grant.effect != EffectClass::ApprovalRequiredMutation || grant.effect != expected_effect
        {
            return Err(ApprovalGrantRejection::WrongEffect);
        }
        if now_unix_ms < grant.issued_at_unix_ms || now_unix_ms >= grant.expires_at_unix_ms {
            return Err(ApprovalGrantRejection::Expired);
        }
        if !consumed_grants.insert(grant.grant_id.clone()) {
            return Err(ApprovalGrantRejection::Replayed);
        }
        Ok(())
    }
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
// Call carries the full approval/trace surface; boxing would change only the
// Rust layout, not JSON, but keep the flat shape so golden/fuzz fixtures stay
// identical to the schema-facing types. Size is bounded by frame limits.
#[allow(clippy::large_enum_variant)]
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

/// Closed `WorkerError.kind` set. RW5 and deserialize reject anything else
/// (`potato`, `ok_validation_lol`). Keep this list exhaustive: a new kind is
/// a protocol change, not a free string.
pub const WORKER_ERROR_KINDS: &[&str] = &[
    "validation",
    "unknown",
    "unsupported",
    "forbidden",
    "sandbox",
    "policy",
    "fixture",
    "deadline_exceeded",
    "timeout",
    "cancelled",
    "output_too_large",
    "frame_too_large",
    "substrate",
    "internal",
    "provider_error",
    "commit_race",
];

pub fn is_typed_worker_error_kind(kind: &str) -> bool {
    WORKER_ERROR_KINDS.contains(&kind)
}

/// Canonicalize a worker error kind.
///
/// Accepted aliases: `deadline` → `deadline_exceeded`, `busy` → `timeout`.
/// Anything else outside [`WORKER_ERROR_KINDS`] is rejected so constructors
/// cannot emit `potato` / `busy` / `deadline`.
pub fn canonical_worker_error_kind(kind: &str) -> Result<&str, String> {
    if kind == "deadline" {
        return Ok("deadline_exceeded");
    }
    if kind == "busy" {
        return Ok("timeout");
    }
    if is_typed_worker_error_kind(kind) {
        return Ok(kind);
    }
    Err(format!(
        "unknown WorkerError.kind {kind:?}; not in the closed RW5 set"
    ))
}

/// Exact names the RW10 harness probes. Fixture, in-process adapters, and
/// the conformance loop must refuse this set.
pub const RW10_FORBIDDEN_OPS: &[&str] = &[
    "planner",
    "planner.run",
    "js.execute",
    "mcp.tools_call",
    "mcp.tools_list",
    "codemode.execute",
    "execute_code",
];

/// Planner / JS / MCP / nested-CodeMode ops a raw worker cannot own.
///
/// Includes [`RW10_FORBIDDEN_OPS`] plus documented aliases so fixture names
/// (`javascript`, `mcp_catalog`) and in-process names (`fz_execute_code`,
/// `tools/call`) stay one arrangement instead of three copied arrays.
///
/// Alias row (not a second law):
/// - `plan` / `planner.*` ≡ `planner`
/// - `js` / `javascript` / `javascript_runtime` / `javascript.*` / `js.*` ≡ `js.execute`
/// - `mcp` / `mcp_catalog` / `mcp.*` / `tools/call` / `tools/list` ≡ `mcp.tools_*`
/// - `codemode` / `codemode.*` / `nested_codemode` / `*_codemode_search` /
///   `*_codemode_describe` ≡ `codemode.execute`
/// - `*_execute_code` / `fszero.exec` ≡ `execute_code`
pub fn is_rw10_forbidden_op(op: &str) -> bool {
    let lower = op.to_ascii_lowercase();
    if RW10_FORBIDDEN_OPS.iter().any(|name| *name == lower) {
        return true;
    }
    matches!(
        lower.as_str(),
        "plan"
            | "js"
            | "javascript"
            | "javascript_runtime"
            | "mcp"
            | "mcp_catalog"
            | "codemode"
            | "nested_codemode"
            | "tools/call"
            | "tools/list"
            | "fszero.exec"
            | "codemode_search"
            | "codemode_describe"
    ) || lower.starts_with("planner.")
        || lower.starts_with("javascript.")
        || lower.starts_with("mcp.")
        || lower.starts_with("js.")
        || lower.starts_with("codemode.")
        || lower.ends_with("_execute_code")
        || lower.ends_with("_codemode_search")
        || lower.ends_with("_codemode_describe")
}

fn deserialize_typed_kind<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let kind = String::deserialize(deserializer)?;
    if is_typed_worker_error_kind(&kind) {
        Ok(kind)
    } else {
        Err(serde::de::Error::custom(format!(
            "unknown WorkerError.kind {kind:?}; not in the closed RW5 set"
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerError {
    #[serde(deserialize_with = "deserialize_typed_kind")]
    pub kind: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl WorkerError {
    /// Construct a typed worker error. Rejects kinds outside
    /// [`WORKER_ERROR_KINDS`] after the `deadline` / `busy` aliases.
    pub fn new(
        kind: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Result<Self, String> {
        let kind = canonical_worker_error_kind(&kind.into())?.to_owned();
        Ok(Self {
            kind,
            message: message.into(),
            retryable,
            details: None,
        })
    }
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        engine_timeline: Option<EngineStageTimelineV1>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker_token_accounting: Option<WorkerTokenAccountingV1>,
    },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        error: WorkerError,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace: Option<WorkerTrace>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        engine_timeline: Option<EngineStageTimelineV1>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker_token_accounting: Option<WorkerTokenAccountingV1>,
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
    let frame: WorkerResponseFrame = serde_json::from_slice(line)
        .map_err(|error| FrameCodecError::InvalidJson(error.to_string()))?;
    reject_shutdown_ack_unknown_fields(line, &frame)?;
    validate_response_frame(&frame)?;
    Ok(frame)
}

// Serde's internally tagged unit variants accept and discard extra object
// fields even when the enum uses `deny_unknown_fields`. Keep the public Rust
// variant and frozen wire shape unchanged, but enforce the empty payload here.
fn reject_shutdown_ack_unknown_fields(
    line: &[u8],
    frame: &WorkerResponseFrame,
) -> Result<(), FrameCodecError> {
    if !matches!(frame, WorkerResponseFrame::ShutdownAck) {
        return Ok(());
    }
    let fields: serde_json::Map<String, Value> = serde_json::from_slice(line)
        .map_err(|error| FrameCodecError::InvalidJson(error.to_string()))?;
    if let Some(field) = fields.keys().find(|field| field.as_str() != "kind") {
        return Err(FrameCodecError::InvalidJson(format!(
            "unknown field `{field}` in shutdown_ack frame"
        )));
    }
    Ok(())
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

pub fn validate_engine_stage_timeline_v1(
    timeline: &EngineStageTimelineV1,
) -> Result<(), FrameCodecError> {
    if timeline.total_ns == 0 || timeline.spans.is_empty() {
        return Err(FrameCodecError::InvalidContract(
            "engine timeline requires non-zero total_ns and at least one span".into(),
        ));
    }
    if timeline.spans.len() > ENGINE_TIMELINE_MAX_SPANS_V1 {
        return Err(FrameCodecError::InvalidContract(format!(
            "engine timeline has {} spans; maximum is {ENGINE_TIMELINE_MAX_SPANS_V1}",
            timeline.spans.len()
        )));
    }
    let mut prior_end = 0_u64;
    let mut duration_sum = 0_u64;
    for span in &timeline.spans {
        require_nonempty("engine_timeline.spans.stage", &span.stage)?;
        if span.duration_ns == 0 {
            return Err(FrameCodecError::InvalidContract(format!(
                "engine timeline span {} has zero duration_ns",
                span.stage
            )));
        }
        if span.start_ns < prior_end {
            return Err(FrameCodecError::InvalidContract(format!(
                "engine timeline span {} overlaps or is out of order",
                span.stage
            )));
        }
        prior_end = span.start_ns.checked_add(span.duration_ns).ok_or_else(|| {
            FrameCodecError::InvalidContract("engine timeline span end overflow".into())
        })?;
        duration_sum = duration_sum.checked_add(span.duration_ns).ok_or_else(|| {
            FrameCodecError::InvalidContract("engine timeline duration sum overflow".into())
        })?;
    }
    if duration_sum.abs_diff(timeline.total_ns) > TIMELINE_CLOSURE_TOLERANCE_NS_V1
        || prior_end
            > timeline
                .total_ns
                .saturating_add(TIMELINE_CLOSURE_TOLERANCE_NS_V1)
    {
        return Err(FrameCodecError::InvalidContract(format!(
            "engine timeline does not close: total_ns={} duration_sum={} final_end_ns={} tolerance_ns={TIMELINE_CLOSURE_TOLERANCE_NS_V1}",
            timeline.total_ns, duration_sum, prior_end
        )));
    }
    Ok(())
}

pub fn validate_worker_token_accounting_v1(
    accounting: &WorkerTokenAccountingV1,
) -> Result<(), FrameCodecError> {
    let tokenizer_id = accounting.tokenizer_id.trim();
    if tokenizer_id.is_empty() || tokenizer_id.len() > 256 {
        return Err(FrameCodecError::InvalidContract(
            "worker token accounting tokenizer_id must be 1..=256 bytes".into(),
        ));
    }
    if let Some(digest) = &accounting.tokenizer_version_digest {
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(FrameCodecError::InvalidContract(
                "worker token accounting tokenizer_version_digest must be 64 lowercase hex".into(),
            ));
        }
    }
    if accounting.cached_tokens > accounting.billed_tokens {
        return Err(FrameCodecError::InvalidContract(
            "worker token accounting cached_tokens exceeds billed_tokens".into(),
        ));
    }
    if tokenizer_id.starts_with("estimator:")
        && accounting.count_kind != WorkerTokenCountKind::Estimate
    {
        return Err(FrameCodecError::InvalidContract(
            "estimator tokenizer identities require count_kind=estimate".into(),
        ));
    }
    Ok(())
}

pub fn validate_response_frame(frame: &WorkerResponseFrame) -> Result<(), FrameCodecError> {
    match frame {
        WorkerResponseFrame::HandshakeAck { .. } | WorkerResponseFrame::ShutdownAck => Ok(()),
        WorkerResponseFrame::Result {
            request_id,
            result,
            engine_timeline,
            worker_token_accounting,
        } => {
            require_nonempty("result.request_id", request_id)?;
            validate_trace(&result.metadata.trace)?;
            if result.metadata.trace.request_id != request_id.as_str() {
                return Err(handshake_field_mismatch(
                    "result.trace.request_id",
                    request_id,
                    &result.metadata.trace.request_id,
                ));
            }
            if let Some(timeline) = engine_timeline {
                validate_engine_stage_timeline_v1(timeline)?;
            }
            if let Some(accounting) = worker_token_accounting {
                validate_worker_token_accounting_v1(accounting)?;
            }
            Ok(())
        }
        WorkerResponseFrame::Error {
            request_id,
            error,
            trace,
            engine_timeline,
            worker_token_accounting,
        } => {
            require_nonempty("error.kind", &error.kind)?;
            require_nonempty("error.message", &error.message)?;
            if let Some(request_id) = request_id {
                require_nonempty("error.request_id", request_id)?;
            }
            if let Some(trace) = trace {
                validate_trace(trace)?;
                if let Some(request_id) = request_id
                    && trace.request_id != request_id.as_str()
                {
                    return Err(handshake_field_mismatch(
                        "error.trace.request_id",
                        request_id,
                        &trace.request_id,
                    ));
                }
            }
            if let Some(timeline) = engine_timeline {
                validate_engine_stage_timeline_v1(timeline)?;
            }
            if let Some(accounting) = worker_token_accounting {
                validate_worker_token_accounting_v1(accounting)?;
            }
            Ok(())
        }
        WorkerResponseFrame::CancelAck { request_id, .. } => {
            require_nonempty("cancel_ack.request_id", request_id)
        }
    }
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
            validate_trace(&request.trace)?;
            if request.request_id != request.trace.request_id {
                return Err(handshake_field_mismatch(
                    "call.trace.request_id",
                    &request.request_id,
                    &request.trace.request_id,
                ));
            }
            Ok(())
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
    if let Some(expected) = expected
        && expected != actual
    {
        return Err(handshake_field_mismatch(field, expected, actual));
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

/// Serialized field names of an exemplar value, in stable lexical order.
fn field_names<T: Serialize>(exemplar: &T) -> Vec<String> {
    let mut names = match serde_json::to_value(exemplar) {
        Ok(Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    };
    // serde_json/preserve_order is feature-unified by downstream workspaces.
    // Contract array order must not depend on whether that feature is enabled.
    names.sort_unstable();
    names
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
        engine: EngineIdentity::FsZero,
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
        approval_grant: None,
        telemetry_request: Some(TelemetryRequestV1 {
            engine_stage_timeline: true,
            worker_token_accounting: true,
        }),
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
            engine: EngineIdentity::FsZero,
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
        expected_engine: EngineIdentity::FsZero,
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
    let timeline = EngineStageTimelineV1 {
        total_ns: 1,
        spans: vec![EngineStageSpanV1 {
            stage: String::new(),
            start_ns: 0,
            duration_ns: 1,
        }],
    };
    let token_accounting = WorkerTokenAccountingV1 {
        tokenizer_id: String::new(),
        tokenizer_version_digest: None,
        count_kind: WorkerTokenCountKind::Exact,
        raw_tokens: 0,
        visible_tokens: 0,
        recovery_tokens: 0,
        billed_tokens: 0,
        cached_tokens: 0,
        exact_ref_tokens: Some(0),
    };
    let result_frame = WorkerResponseFrame::Result {
        request_id: String::new(),
        result: WorkerResult {
            value: Value::Null,
            metadata: result_metadata.clone(),
        },
        engine_timeline: Some(timeline.clone()),
        worker_token_accounting: Some(token_accounting.clone()),
    };
    let error_frame = WorkerResponseFrame::Error {
        request_id: Some(String::new()),
        error: WorkerError {
            kind: String::new(),
            message: String::new(),
            retryable: false,
            details: Some(Value::Null),
        },
        trace: Some(manifest_trace()),
        engine_timeline: Some(timeline.clone()),
        worker_token_accounting: Some(token_accounting.clone()),
    };

    json!({
        "protocol_version": RAW_WORKER_PROTOCOL_VERSION,
        "linked_contracts": {
            "assembly_abi_contract_version": ASSEMBLY_ABI_CONTRACT_VERSION,
            "assembly_manifest_schema_version": ASSEMBLY_MANIFEST_SCHEMA_VERSION,
            "assembly_abi_contract_digest": assembly_abi_contract_digest_v1(),
                "robust_snap_contract_version": ROBUST_SNAP_CONTRACT_VERSION,
                "robust_snap_model_version": ROBUST_SNAP_MODEL_VERSION,
                "robust_snap_contract_digest": robust_snap_contract_digest_v1(),
        },
        "framing": "bounded_ndjson",
        "default_max_frame_bytes": DEFAULT_MAX_FRAME_BYTES,
        "request_frames": ["handshake", "call", "cancel", "shutdown"],
        "response_frames": ["handshake_ack", "result", "error", "cancel_ack", "shutdown_ack"],
        "binding": field_names(&binding),
        "handshake_request": field_names(&handshake),
        "capabilities": field_names(&capabilities),
        "limits": field_names(&ProtocolLimits::default()),
        "call": field_names(&call),
        "telemetry_request": field_names(&TelemetryRequestV1 {
            engine_stage_timeline: true,
            worker_token_accounting: true,
        }),
        "engine_stage_span": field_names(&timeline.spans[0]),
        "engine_stage_timeline": field_names(&timeline),
        "worker_token_accounting": field_names(&token_accounting),
        "worker_token_count_kinds": ["exact", "conservative_upper_bound", "estimate"],
        "result_frame": field_names(&result_frame),
        "error_frame": field_names(&error_frame),
        "trace": field_names(&manifest_trace()),
        "result_metadata": field_names(&result_metadata),
        "negative_space": ["planner", "javascript_runtime", "mcp_catalog", "nested_codemode"],
    })
}

pub fn raw_worker_protocol_digest_hex() -> String {
    contract_digest_hex(&raw_worker_protocol_manifest())
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-abi/unit/raw_worker.rs"]
mod tests;
