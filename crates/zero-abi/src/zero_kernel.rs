//! Canonical, versionless ZeroKernel contracts.
//!
//! ZeroKernel is the only model-facing execution surface. Domain engines
//! implement the typed traits in this module; models never select an engine,
//! transport, operation registry, or ref owner.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable wire identity. Evolution is bound by `contract_digest`, never by a
/// numeric suffix in a symbol or operation name.
pub const ZERO_KERNEL_PROTOCOL: &str = "ZeroKernel";
pub const ZERO_HANDLE_PREFIX: &str = "z://blob/";
pub const HANDLE_DIGEST_BYTES: usize = 64;
pub const SOURCE_BYTE_LIMIT: usize = 256 * 1024;
pub const STATE_KEY_LIMIT: usize = 64;
pub const STATE_KEY_BYTE_LIMIT: usize = 128;
pub const STATE_VALUE_BYTE_LIMIT: usize = 4 * 1024;
pub const STATE_TOTAL_BYTE_LIMIT: usize = 16 * 1024;
pub const PARALLEL_TASK_LIMIT: usize = 16;
pub const PIPELINE_STAGE_LIMIT: usize = 16;
pub const OPERATION_TRACE_LIMIT: usize = 128;

/// The complete direct guest catalog. This list drives declarations and
/// introspection; it is not an engine operation registry.
pub const GUEST_METHODS: &[&str] = &[
    "read",
    "find",
    "edit",
    "apply",
    "run",
    // Compatibility aliases: kept callable but omitted from z.help()'s
    // canonical six-operation surface.
    "snap",
    "write",
    "effect",
    "remove",
    "transact",
    "asgrep",
    "lookup",
    "parallel",
    "pipeline",
    "shell",
    "measure",
    "project",
    "compress",
    "expand",
    "state.get",
    "state.set",
    "state.has",
    "state.delete",
    "state.list",
    "help",
    "inspect",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KernelBudget {
    pub wall_ms: u64,
    pub cpu_ms: u64,
    pub memory_bytes: u64,
    pub call_limit: u32,
    pub task_limit: u32,
    pub output_byte_limit: u32,
}

impl KernelBudget {
    pub fn validate(&self) -> Result<(), ZeroKernelError> {
        if self.wall_ms == 0
            || self.cpu_ms == 0
            || self.memory_bytes == 0
            || self.call_limit == 0
            || self.task_limit == 0
            || self.output_byte_limit == 0
        {
            return Err(ZeroKernelError::InvalidBudget(
                "every budget dimension must be finite and positive".into(),
            ));
        }
        if self.task_limit as usize > PARALLEL_TASK_LIMIT {
            return Err(ZeroKernelError::InvalidBudget(format!(
                "task_limit exceeds {PARALLEL_TASK_LIMIT}"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KernelContext {
    pub workspace_root: PathBuf,
    pub project_root: PathBuf,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_state_root: Option<String>,
    pub contract_digest: String,
}

impl KernelContext {
    pub fn validate(&self) -> Result<(), ZeroKernelError> {
        if self.session_id.is_empty() {
            return Err(ZeroKernelError::InvalidContext(
                "session_id must not be empty".into(),
            ));
        }
        if self.contract_digest.is_empty() {
            return Err(ZeroKernelError::InvalidContext(
                "contract_digest must not be empty".into(),
            ));
        }
        if !self.workspace_root.is_absolute() || !self.project_root.is_absolute() {
            return Err(ZeroKernelError::InvalidContext(
                "workspace_root and project_root must be absolute".into(),
            ));
        }
        Ok(())
    }
}

/// Opaque BLAKE3 content handle. Models pass this value to `z.expand`; they do
/// not inspect or route by producer.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ZeroHandle(String);

impl ZeroHandle {
    pub fn parse(value: impl Into<String>) -> Result<Self, ZeroKernelError> {
        let value = value.into();
        let digest = value
            .strip_prefix(ZERO_HANDLE_PREFIX)
            .ok_or_else(|| ZeroKernelError::InvalidHandle("expected z://blob handle".into()))?;
        if digest.len() != HANDLE_DIGEST_BYTES
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ZeroKernelError::InvalidHandle(
                "handle digest must be 64 lowercase hexadecimal characters".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn from_digest(digest: &str) -> Result<Self, ZeroKernelError> {
        Self::parse(format!("{ZERO_HANDLE_PREFIX}{digest}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn digest(&self) -> &str {
        &self.0[ZERO_HANDLE_PREFIX.len()..]
    }
}

impl fmt::Display for ZeroHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ZeroKernelOutcome {
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct KernelLedger {
    pub wall_ns: u64,
    pub cpu_ns_upper_bound: u64,
    pub calls: u32,
    pub tasks: u32,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub bytes_visible: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StateEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    pub unchanged: bool,
}

impl StateEvidence {
    pub fn validate(&self, outcome: &ZeroKernelOutcome) -> Result<(), ZeroKernelError> {
        if self.unchanged != (self.before == self.after) {
            return Err(ZeroKernelError::InvalidResponse(
                "state unchanged flag disagrees with roots".into(),
            ));
        }
        if matches!(
            outcome,
            ZeroKernelOutcome::Cancelled | ZeroKernelOutcome::Failed
        ) && !self.unchanged
        {
            return Err(ZeroKernelError::InvalidResponse(
                "cancelled or failed execution changed the durable state root".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroKernelRequest {
    pub protocol: String,
    pub source: String,
    pub context: KernelContext,
    pub budget: KernelBudget,
}

impl ZeroKernelRequest {
    pub fn new(
        source: String,
        context: KernelContext,
        budget: KernelBudget,
    ) -> Result<Self, ZeroKernelError> {
        let request = Self {
            protocol: ZERO_KERNEL_PROTOCOL.into(),
            source,
            context,
            budget,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), ZeroKernelError> {
        if self.protocol != ZERO_KERNEL_PROTOCOL {
            return Err(ZeroKernelError::InvalidProtocol(self.protocol.clone()));
        }
        if self.source.is_empty() || self.source.len() > SOURCE_BYTE_LIMIT {
            return Err(ZeroKernelError::InvalidSource(format!(
                "source must be 1..={SOURCE_BYTE_LIMIT} bytes"
            )));
        }
        self.context.validate()?;
        self.budget.validate()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZeroOperationStatus {
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ZeroOperationTrace {
    pub sequence: u64,
    pub method: String,
    pub status: ZeroOperationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_group: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_files: Option<u32>,
    pub duration_ns: u64,
}

impl ZeroOperationTrace {
    fn validate(&self) -> Result<(), ZeroKernelError> {
        if self.sequence == 0 || !GUEST_METHODS.contains(&self.method.as_str()) {
            return Err(ZeroKernelError::InvalidResponse(
                "operation trace contains an invalid sequence or method".into(),
            ));
        }
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.is_empty() || target.len() > 1_024)
            || self
                .detail
                .as_ref()
                .is_some_and(|detail| detail.is_empty() || detail.len() > 1_024)
        {
            return Err(ZeroKernelError::InvalidResponse(
                "operation trace target or detail is invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ZeroKernelResponse {
    pub protocol: String,
    pub outcome: ZeroKernelOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EngineError>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<ZeroOperationTrace>,
    #[serde(default)]
    pub operations_truncated: bool,
    #[serde(default)]
    pub handles: Vec<ZeroHandle>,
    pub event: ZeroHandle,
    pub state: StateEvidence,
    pub ledger: KernelLedger,
}

impl ZeroKernelResponse {
    pub fn validate(&self) -> Result<(), ZeroKernelError> {
        if self.protocol != ZERO_KERNEL_PROTOCOL {
            return Err(ZeroKernelError::InvalidProtocol(self.protocol.clone()));
        }
        match (&self.outcome, &self.error) {
            (ZeroKernelOutcome::Completed, None) => {}
            (ZeroKernelOutcome::Cancelled, Some(error))
                if error.kind == EngineErrorKind::Cancelled => {}
            (ZeroKernelOutcome::Failed, Some(_)) => {}
            (ZeroKernelOutcome::Completed, Some(_)) => {
                return Err(ZeroKernelError::InvalidResponse(
                    "completed response carries an error".into(),
                ));
            }
            (ZeroKernelOutcome::Cancelled, _) => {
                return Err(ZeroKernelError::InvalidResponse(
                    "cancelled response requires a cancelled error".into(),
                ));
            }
            (ZeroKernelOutcome::Failed, None) => {
                return Err(ZeroKernelError::InvalidResponse(
                    "failed response has no typed error".into(),
                ));
            }
        }
        if self.operations.len() > OPERATION_TRACE_LIMIT {
            return Err(ZeroKernelError::InvalidResponse(
                "operation trace exceeds its bounded limit".into(),
            ));
        }
        let mut previous_sequence = 0;
        for operation in &self.operations {
            operation.validate()?;
            if operation.sequence <= previous_sequence {
                return Err(ZeroKernelError::InvalidResponse(
                    "operation trace sequence is not strictly increasing".into(),
                ));
            }
            previous_sequence = operation.sequence;
        }
        self.state.validate(&self.outcome)
    }
}

/// Append-only record from which every model-visible byte is rendered.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ZeroKernelEvent {
    pub protocol: String,
    pub session_id: String,
    pub cell_id: String,
    pub source_digest: String,
    pub contract_digest: String,
    pub policy_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_root_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_root_after: Option<String>,
    #[serde(default)]
    pub input_handles: Vec<ZeroHandle>,
    #[serde(default)]
    pub output_handles: Vec<ZeroHandle>,
    pub outcome: ZeroKernelOutcome,
    pub ledger: KernelLedger,
    pub model_visible_digest: String,
}

impl ZeroKernelEvent {
    pub fn validate(&self) -> Result<(), ZeroKernelError> {
        if self.protocol != ZERO_KERNEL_PROTOCOL {
            return Err(ZeroKernelError::InvalidProtocol(self.protocol.clone()));
        }
        for (name, value) in [
            ("session_id", &self.session_id),
            ("cell_id", &self.cell_id),
            ("source_digest", &self.source_digest),
            ("contract_digest", &self.contract_digest),
            ("policy_digest", &self.policy_digest),
            ("model_visible_digest", &self.model_visible_digest),
        ] {
            if value.is_empty() {
                return Err(ZeroKernelError::InvalidEvent(format!(
                    "{name} must not be empty"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineErrorKind {
    InvalidInput,
    OutsideWorkspace,
    NotFound,
    Conflict,
    Cancelled,
    Deadline,
    Budget,
    Unsupported,
    Corrupt,
    Io,
    Internal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineError {
    pub kind: EngineErrorKind,
    pub detail: String,
    pub retryable: bool,
}

impl EngineError {
    pub fn new(kind: EngineErrorKind, detail: impl Into<String>, retryable: bool) -> Self {
        Self {
            kind,
            detail: detail.into(),
            retryable,
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for EngineError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineCallContext {
    pub workspace_root: PathBuf,
    pub project_root: PathBuf,
    pub session_id: String,
    pub cell_id: String,
    pub trace_id: String,
    pub deadline_unix_ms: u64,
    pub budget: KernelBudget,
}

/// Call-scoped cancellation probe. Engines must poll at their documented
/// bounded work boundaries.
pub trait CancellationProbe: Send + Sync {
    fn is_cancelled(&self) -> bool;

    fn atomic_flag(&self) -> Option<Arc<AtomicBool>> {
        None
    }
}

#[derive(Clone)]
pub struct EngineInvocation {
    pub context: EngineCallContext,
    pub cancellation: Arc<dyn CancellationProbe>,
}

impl fmt::Debug for EngineInvocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EngineInvocation")
            .field("context", &self.context)
            .field("cancelled", &self.cancellation.is_cancelled())
            .finish()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileReadRequest {
    pub path: PathBuf,
    pub options: ReadOptions,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileSnapshot {
    pub path: PathBuf,
    pub content: ZeroHandle,
    pub byte_len: u64,
    pub modified_unix_ns: u128,
    pub mode: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_utf8: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileMetadata {
    pub mode: u32,
    pub modified_unix_ns: u128,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileEffectKind {
    Write,
    Edit,
    Remove,
    Restore,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileEffectRequest {
    pub kind: FileEffectKind,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_preimage: Option<ZeroHandle>,
    #[serde(default)]
    pub expect_absent: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileEffectReceipt {
    pub kind: FileEffectKind,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ZeroHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ZeroHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_metadata: Option<FileMetadata>,
    pub journal: ZeroHandle,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LookupOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

pub trait FileLease: Send {}

pub trait FileEngine: Send + Sync {
    fn lease(&self, invocation: &EngineInvocation) -> Result<Box<dyn FileLease>, EngineError>;

    fn read(
        &self,
        invocation: &EngineInvocation,
        request: FileReadRequest,
    ) -> Result<FileSnapshot, EngineError>;

    fn lookup(
        &self,
        invocation: &EngineInvocation,
        root: PathBuf,
        options: LookupOptions,
    ) -> Result<Vec<PathBuf>, EngineError>;

    fn apply(
        &self,
        invocation: &EngineInvocation,
        request: FileEffectRequest,
    ) -> Result<FileEffectReceipt, EngineError>;

    fn restore(
        &self,
        invocation: &EngineInvocation,
        receipt: &FileEffectReceipt,
    ) -> Result<(), EngineError>;

    fn reconcile(&self, invocation: &EngineInvocation) -> Result<Vec<ZeroHandle>, EngineError>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AsgrepMode {
    Natural,
    Pattern,
    Word,
    Literal,
    Regex,
    Imports,
    Symbols,
    #[serde(alias = "defs")]
    Definition,
    References,
    Callers,
    Callees,
    #[serde(alias = "call-path")]
    CallPath,
    Semantic,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AsgrepOptions {
    pub mode: AsgrepMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sink: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralQuery {
    pub query: String,
    pub options: AsgrepOptions,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StructuralHit {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<ZeroHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ZeroHandle>,
    pub score: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StructuralCoverage {
    pub tier_a_pct: f64,
    pub tier_b_pct: f64,
    pub tier_c_pct: f64,
    pub freshness_verified: bool,
    pub snapshot_id: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StructuralAbsence {
    pub class: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<StructuralCoverage>,
    pub suggestion: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StructuralBudget {
    pub requested: u32,
    pub used: u32,
    pub actual_used: u32,
    pub remaining: u32,
    pub exceeded: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StructuralResult {
    pub hits: Vec<StructuralHit>,
    pub index_digest: String,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<StructuralCoverage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absence: Option<StructuralAbsence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<StructuralBudget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ZeroHandle>,
}

pub trait StructuralEngine: Send + Sync {
    fn query(
        &self,
        invocation: &EngineInvocation,
        query: StructuralQuery,
    ) -> Result<StructuralResult, EngineError>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TokenAccounting {
    pub tokenizer: String,
    pub billed: u64,
    pub visible: u64,
    pub cached: u64,
    pub certified: bool,
}

/// Result of re-measuring bytes against a previously claimed accounting.
/// `matches` is true only when every claimed field equals the recomputation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CertifyResult {
    pub matches: bool,
    pub recomputed: TokenAccounting,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionRequest {
    pub bytes: Vec<u8>,
    pub visible_byte_limit: u32,
    pub media_type: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProjectionResult {
    pub visible: String,
    pub visible_source_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact: Option<ZeroHandle>,
    pub accounting: TokenAccounting,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompressionRequest {
    pub bytes: Vec<u8>,
    pub max_tokens: u32,
    #[serde(default)]
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub media_type: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompressionResult {
    pub visible: String,
    pub exact: ZeroHandle,
    pub truncated: bool,
    pub omitted_tokens: u64,
    pub accounting: TokenAccounting,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpandOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShellOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_visible_bytes: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ShellResult {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact: Option<ZeroHandle>,
    pub accounting: TokenAccounting,
}

pub trait TokenEngine: Send + Sync {
    fn measure(
        &self,
        invocation: &EngineInvocation,
        bytes: &[u8],
    ) -> Result<TokenAccounting, EngineError>;

    /// Re-measure `bytes` and compare against `claimed`. The kernel response
    /// boundary uses this to prove that reported accounting equals reality
    /// before an event may claim certified=true (RACC truthfulness).
    fn certify(
        &self,
        invocation: &EngineInvocation,
        bytes: &[u8],
        claimed: &TokenAccounting,
    ) -> Result<CertifyResult, EngineError>;

    fn project(
        &self,
        invocation: &EngineInvocation,
        request: ProjectionRequest,
    ) -> Result<ProjectionResult, EngineError>;

    fn compress(
        &self,
        invocation: &EngineInvocation,
        request: CompressionRequest,
    ) -> Result<CompressionResult, EngineError>;

    fn expand(
        &self,
        invocation: &EngineInvocation,
        handle: &ZeroHandle,
        options: ExpandOptions,
    ) -> Result<Vec<u8>, EngineError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ZeroKernelError {
    InvalidProtocol(String),
    InvalidSource(String),
    InvalidBudget(String),
    InvalidContext(String),
    InvalidHandle(String),
    InvalidResponse(String),
    InvalidEvent(String),
}

impl fmt::Display for ZeroKernelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProtocol(value) => {
                write!(formatter, "invalid ZeroKernel protocol: {value}")
            }
            Self::InvalidSource(detail) => write!(formatter, "invalid ZeroKernel source: {detail}"),
            Self::InvalidBudget(detail) => write!(formatter, "invalid ZeroKernel budget: {detail}"),
            Self::InvalidContext(detail) => {
                write!(formatter, "invalid ZeroKernel context: {detail}")
            }
            Self::InvalidHandle(detail) => write!(formatter, "invalid ZeroHandle: {detail}"),
            Self::InvalidResponse(detail) => {
                write!(formatter, "invalid ZeroKernel response: {detail}")
            }
            Self::InvalidEvent(detail) => write!(formatter, "invalid ZeroKernel event: {detail}"),
        }
    }
}

impl std::error::Error for ZeroKernelError {}
