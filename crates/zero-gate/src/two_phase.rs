//! Assembly-bound two-phase execution kernel.
//!
//! `ExecutionPermit`, `BrokeredExecution`, `ReadyToFinalize`, and the final
//! receipts are linear capabilities. Their fields are private, they are not
//! cloneable, and only the preceding phase can construct the next phase.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use zero_abi::raw_worker::EffectClass;

pub const TWO_PHASE_SCHEMA_VERSION: u16 = 1;
pub const GUARD_COUNT: usize = 10;
pub const MAX_SOURCE_REPOSITORIES: usize = 64;
pub const MAX_CONTROLLER_INSTRUCTIONS: usize = 4_096;

pub type DigestV1 = [u8; 32];

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Guard {
    G0Canonical = 0,
    G1Coherence = 1,
    G2FinitePlan = 2,
    G3Attribution = 3,
    G4Resources = 4,
    G5RobustSnap = 5,
    G6SafetyShield = 6,
    G7Performance = 7,
    G8TransactionClosure = 8,
    G9ReceiptCommitment = 9,
}

impl Guard {
    pub const ALL: [Self; GUARD_COUNT] = [
        Self::G0Canonical,
        Self::G1Coherence,
        Self::G2FinitePlan,
        Self::G3Attribution,
        Self::G4Resources,
        Self::G5RobustSnap,
        Self::G6SafetyShield,
        Self::G7Performance,
        Self::G8TransactionClosure,
        Self::G9ReceiptCommitment,
    ];

    pub fn predecessor(self) -> Option<Self> {
        let index = self as usize;
        index.checked_sub(1).map(|previous| Self::ALL[previous])
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardStatus {
    Passed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuardEvent {
    pub guard: Guard,
    pub predecessor: Option<Guard>,
    pub status: GuardStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum FailureCode {
    SchemaVersionMismatch,
    MissingBinding,
    InvalidSourceIdentity,
    CanonicalDigestMismatch,
    CoherenceFailure,
    InvalidPlan,
    PlanDigestMismatch,
    SemanticCutCrossing,
    AttributionChanged,
    UnboundedWorker,
    BoundExceeded,
    MissingSnapCertificate,
    MissingSafetyShield,
    IrreversiblePreEvidenceEffect,
    PerformanceUnknown,
    ExecuteWithoutPermit,
    ForgedPermit,
    PlanStepMismatch,
    BufferOverflow,
    EarlyVisibleByte,
    IncompleteExecution,
    IncompleteTrace,
    ForgedPredecessor,
    IncompleteTransactionClosure,
    UnaccountedFallback,
    MissingApprovalGrant,
    ForgedReceipt,
}

impl FailureCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SchemaVersionMismatch => "schema_version_mismatch",
            Self::MissingBinding => "missing_binding",
            Self::InvalidSourceIdentity => "invalid_source_identity",
            Self::CanonicalDigestMismatch => "canonical_digest_mismatch",
            Self::CoherenceFailure => "coherence_failure",
            Self::InvalidPlan => "invalid_plan",
            Self::PlanDigestMismatch => "plan_digest_mismatch",
            Self::SemanticCutCrossing => "semantic_cut_crossing",
            Self::AttributionChanged => "attribution_changed",
            Self::UnboundedWorker => "unbounded_worker",
            Self::BoundExceeded => "bound_exceeded",
            Self::MissingSnapCertificate => "missing_snap_certificate",
            Self::MissingSafetyShield => "missing_safety_shield",
            Self::IrreversiblePreEvidenceEffect => "irreversible_pre_evidence_effect",
            Self::PerformanceUnknown => "performance_unknown",
            Self::ExecuteWithoutPermit => "execute_without_permit",
            Self::ForgedPermit => "forged_permit",
            Self::PlanStepMismatch => "plan_step_mismatch",
            Self::BufferOverflow => "buffer_overflow",
            Self::EarlyVisibleByte => "early_visible_byte",
            Self::IncompleteExecution => "incomplete_execution",
            Self::IncompleteTrace => "incomplete_trace",
            Self::ForgedPredecessor => "forged_predecessor",
            Self::IncompleteTransactionClosure => "incomplete_transaction_closure",
            Self::UnaccountedFallback => "unaccounted_fallback",
            Self::MissingApprovalGrant => "missing_approval_grant",
            Self::ForgedReceipt => "forged_receipt",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KernelError {
    pub code: FailureCode,
    pub guard: Option<Guard>,
    pub detail: String,
}

impl KernelError {
    fn at(code: FailureCode, guard: Guard, detail: impl Into<String>) -> Self {
        Self {
            code,
            guard: Some(guard),
            detail: detail.into(),
        }
    }

    fn execution(code: FailureCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            guard: None,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}
impl std::error::Error for KernelError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionTrace {
    events: Vec<GuardEvent>,
    executed_instructions: u32,
    worker_steps: u64,
    buffered_visible_bytes: u64,
    staged_effects: u32,
    execution_failure: Option<FailureCode>,
}

impl ExecutionTrace {
    fn new() -> Self {
        Self {
            events: Vec::with_capacity(GUARD_COUNT),
            executed_instructions: 0,
            worker_steps: 0,
            buffered_visible_bytes: 0,
            staged_effects: 0,
            execution_failure: None,
        }
    }

    fn pass(&mut self, guard: Guard) {
        self.events.push(GuardEvent {
            guard,
            predecessor: guard.predecessor(),
            status: GuardStatus::Passed,
        });
    }

    pub fn events(&self) -> &[GuardEvent] {
        &self.events
    }
    pub fn executed_instructions(&self) -> u32 {
        self.executed_instructions
    }
    pub fn worker_steps(&self) -> u64 {
        self.worker_steps
    }
    pub fn buffered_visible_bytes(&self) -> u64 {
        self.buffered_visible_bytes
    }
    pub fn staged_effects(&self) -> u32 {
        self.staged_effects
    }
    pub fn execution_failure(&self) -> Option<FailureCode> {
        self.execution_failure
    }

    pub fn verify_prefix(&self) -> Result<(), KernelError> {
        if self.events.len() > GUARD_COUNT {
            return Err(KernelError::execution(
                FailureCode::IncompleteTrace,
                "guard trace exceeds G0-G9",
            ));
        }
        for (index, event) in self.events.iter().enumerate() {
            let expected = Guard::ALL[index];
            if event.guard != expected {
                return Err(KernelError::execution(
                    FailureCode::IncompleteTrace,
                    format!(
                        "expected {expected:?} at index {index}, found {:?}",
                        event.guard
                    ),
                ));
            }
            if event.predecessor != expected.predecessor() {
                return Err(KernelError::execution(
                    FailureCode::ForgedPredecessor,
                    format!("invalid predecessor for {expected:?}"),
                ));
            }
            if event.status != GuardStatus::Passed {
                return Err(KernelError::execution(
                    FailureCode::IncompleteTrace,
                    format!("{expected:?} did not pass"),
                ));
            }
        }
        Ok(())
    }

    pub fn verify_complete(&self) -> Result<(), KernelError> {
        self.verify_prefix()?;
        if self.events.len() != GUARD_COUNT {
            return Err(KernelError::execution(
                FailureCode::IncompleteTrace,
                format!("expected {GUARD_COUNT} guards, found {}", self.events.len()),
            ));
        }
        Ok(())
    }

    pub fn digest(&self) -> DigestV1 {
        let mut bytes = Vec::with_capacity(64 + self.events.len() * 3);
        bytes.extend_from_slice(&TWO_PHASE_SCHEMA_VERSION.to_be_bytes());
        for event in &self.events {
            bytes.push(event.guard as u8);
            bytes.push(event.predecessor.map_or(u8::MAX, |guard| guard as u8));
            bytes.push(match event.status {
                GuardStatus::Passed => 0,
            });
        }
        bytes.extend_from_slice(&self.executed_instructions.to_be_bytes());
        bytes.extend_from_slice(&self.worker_steps.to_be_bytes());
        bytes.extend_from_slice(&self.buffered_visible_bytes.to_be_bytes());
        bytes.extend_from_slice(&self.staged_effects.to_be_bytes());
        bytes.push(self.execution_failure.map_or(u8::MAX, |code| code as u8));
        hash_bytes(&bytes)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceHead {
    pub repository: String,
    pub head: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionBinding {
    pub schema_version: u16,
    pub assembly_manifest_digest: DigestV1,
    pub source_tree_digest: DigestV1,
    pub source_repository_heads: Vec<SourceHead>,
    pub image_digest: DigestV1,
    pub plan_digest: DigestV1,
    pub comparison_identity_digest: DigestV1,
    pub predecessor_receipt_head: DigestV1,
}

impl ExecutionBinding {
    pub fn digest(&self) -> DigestV1 {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.schema_version.to_be_bytes());
        bytes.extend_from_slice(&self.assembly_manifest_digest);
        bytes.extend_from_slice(&self.source_tree_digest);
        for source in &self.source_repository_heads {
            append_bounded(&mut bytes, source.repository.as_bytes());
            append_bounded(&mut bytes, source.head.as_bytes());
        }
        bytes.extend_from_slice(&self.image_digest);
        bytes.extend_from_slice(&self.plan_digest);
        bytes.extend_from_slice(&self.comparison_identity_digest);
        bytes.extend_from_slice(&self.predecessor_receipt_head);
        hash_bytes(&bytes)
    }
}

fn effect_class_tag(effect_class: EffectClass) -> u8 {
    match effect_class {
        EffectClass::ReadOnly => 0,
        EffectClass::ReversibleMutation => 1,
        EffectClass::ApprovalRequiredMutation => 2,
        EffectClass::Irreversible => 3,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum PeerOwner {
    FsZero,
    GraphZero,
    TokenZero,
    ZeroStack,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ExecutionSurface {
    Mcp,
    Cli,
    ClaudeCode,
    Pi,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ControllerInstruction {
    Dispatch { owner: PeerOwner },
    DeterministicTransform,
    Verify,
    StageEffect,
    BufferVisible,
    CloseTransaction,
}

impl ControllerInstruction {
    fn tag(self) -> u8 {
        match self {
            Self::Dispatch { owner } => 0x10 + owner as u8,
            Self::DeterministicTransform => 0x20,
            Self::Verify => 0x30,
            Self::StageEffect => 0x40,
            Self::BufferVisible => 0x50,
            Self::CloseTransaction => 0x60,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPlan {
    pub instructions: Vec<ControllerInstruction>,
}
impl ControllerPlan {
    pub fn digest(&self) -> DigestV1 {
        let mut bytes = Vec::with_capacity(8 + self.instructions.len());
        bytes.extend_from_slice(&(self.instructions.len() as u64).to_be_bytes());
        bytes.extend(
            self.instructions
                .iter()
                .map(|instruction| instruction.tag()),
        );
        hash_bytes(&bytes)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerEnvelope {
    pub fuel: u64,
    pub deadline_ms: u64,
    pub io_bytes: u64,
    pub output_bytes: u64,
    pub memory_bytes: u64,
    pub processes: u32,
    pub risk_units: u64,
    pub worker_steps: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceUsage {
    pub fuel: u64,
    pub elapsed_ms: u64,
    pub io_bytes: u64,
    pub memory_bytes: u64,
    pub processes: u32,
    pub risk_units: u64,
    pub worker_steps: u64,
}

impl ResourceUsage {
    fn checked_add(self, delta: Self) -> Option<Self> {
        Some(Self {
            fuel: self.fuel.checked_add(delta.fuel)?,
            elapsed_ms: self.elapsed_ms.checked_add(delta.elapsed_ms)?,
            io_bytes: self.io_bytes.checked_add(delta.io_bytes)?,
            memory_bytes: self.memory_bytes.max(delta.memory_bytes),
            processes: self.processes.max(delta.processes),
            risk_units: self.risk_units.checked_add(delta.risk_units)?,
            worker_steps: self.worker_steps.checked_add(delta.worker_steps)?,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticAuthority {
    OwnerScoped,
    HiddenTaskSelector,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributionClass {
    Fixed,
    Changed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum SnapEvidence {
    NotClaimed,
    Verified { certificate_digest: DigestV1 },
    ClaimedWithoutCertificate,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum PerformanceAdmission {
    ExactNeutral,
    PointwiseDominance { evidence_digest: DigestV1 },
    ScopedCertificate { evidence_digest: DigestV1 },
    Distributional { evidence_digest: DigestV1 },
    BaselineFallback,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuardEvidence {
    pub canonical_object_digest: DigestV1,
    pub decoded_object_digest: DigestV1,
    pub owner_coherent: bool,
    pub producer_coherent: bool,
    pub schema_coherent: bool,
    pub source_root_coherent: bool,
    pub semantic_authority: SemanticAuthority,
    pub attribution_class: AttributionClass,
    pub snap: SnapEvidence,
    pub safety_shield_digest: DigestV1,
    pub approval_grant_digest: Option<DigestV1>,
    pub irreversible_pre_action_evidence_digest: Option<DigestV1>,
    pub performance: PerformanceAdmission,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareRequest {
    pub binding: ExecutionBinding,
    pub surface: ExecutionSurface,
    pub effect_class: EffectClass,
    pub plan: ControllerPlan,
    pub envelope: WorkerEnvelope,
    pub evidence: GuardEvidence,
}

impl PrepareRequest {
    /// Canonical commitment to every G0-G7 admission input.
    pub fn admission_digest(&self) -> DigestV1 {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.binding.digest());
        bytes.push(self.surface as u8);
        bytes.push(effect_class_tag(self.effect_class));
        bytes.extend_from_slice(&self.plan.digest());

        let envelope = self.envelope;
        bytes.extend_from_slice(&envelope.fuel.to_be_bytes());
        bytes.extend_from_slice(&envelope.deadline_ms.to_be_bytes());
        bytes.extend_from_slice(&envelope.io_bytes.to_be_bytes());
        bytes.extend_from_slice(&envelope.output_bytes.to_be_bytes());
        bytes.extend_from_slice(&envelope.memory_bytes.to_be_bytes());
        bytes.extend_from_slice(&envelope.processes.to_be_bytes());
        bytes.extend_from_slice(&envelope.risk_units.to_be_bytes());
        bytes.extend_from_slice(&envelope.worker_steps.to_be_bytes());

        let evidence = &self.evidence;
        bytes.extend_from_slice(&evidence.canonical_object_digest);
        bytes.extend_from_slice(&evidence.decoded_object_digest);
        bytes.push(evidence.owner_coherent as u8);
        bytes.push(evidence.producer_coherent as u8);
        bytes.push(evidence.schema_coherent as u8);
        bytes.push(evidence.source_root_coherent as u8);
        bytes.push(match evidence.semantic_authority {
            SemanticAuthority::OwnerScoped => 0,
            SemanticAuthority::HiddenTaskSelector => 1,
        });
        bytes.push(match evidence.attribution_class {
            AttributionClass::Fixed => 0,
            AttributionClass::Changed => 1,
        });
        match evidence.snap {
            SnapEvidence::NotClaimed => bytes.push(0),
            SnapEvidence::Verified { certificate_digest } => {
                bytes.push(1);
                bytes.extend_from_slice(&certificate_digest);
            }
            SnapEvidence::ClaimedWithoutCertificate => bytes.push(2),
        }
        bytes.extend_from_slice(&evidence.safety_shield_digest);
        append_optional_digest(&mut bytes, evidence.approval_grant_digest);
        append_optional_digest(&mut bytes, evidence.irreversible_pre_action_evidence_digest);
        match evidence.performance {
            PerformanceAdmission::ExactNeutral => bytes.push(0),
            PerformanceAdmission::PointwiseDominance { evidence_digest } => {
                bytes.push(1);
                bytes.extend_from_slice(&evidence_digest);
            }
            PerformanceAdmission::ScopedCertificate { evidence_digest } => {
                bytes.push(2);
                bytes.extend_from_slice(&evidence_digest);
            }
            PerformanceAdmission::Distributional { evidence_digest } => {
                bytes.push(3);
                bytes.extend_from_slice(&evidence_digest);
            }
            PerformanceAdmission::BaselineFallback => bytes.push(4),
            PerformanceAdmission::Unknown => bytes.push(5),
        }
        hash_bytes(&bytes)
    }
}

#[derive(Debug)]
pub struct PrepareFailure {
    error: KernelError,
    trace: ExecutionTrace,
}
impl PrepareFailure {
    pub fn error(&self) -> &KernelError {
        &self.error
    }
    pub fn trace(&self) -> &ExecutionTrace {
        &self.trace
    }
    pub fn into_parts(self) -> (KernelError, ExecutionTrace) {
        (self.error, self.trace)
    }
}
impl fmt::Display for PrepareFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}
impl std::error::Error for PrepareFailure {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PermitRecord {
    pub schema_version: u16,
    pub permit_id: DigestV1,
    pub binding_digest: DigestV1,
    pub admission_digest: DigestV1,
    pub surface: ExecutionSurface,
    pub trace: ExecutionTrace,
}

/// Opaque, linear execution authority. It cannot be deserialized or cloned.
#[derive(Debug)]
pub struct ExecutionPermit {
    permit_id: DigestV1,
    request: PrepareRequest,
    trace: ExecutionTrace,
}

impl ExecutionPermit {
    pub fn record(&self) -> PermitRecord {
        PermitRecord {
            schema_version: TWO_PHASE_SCHEMA_VERSION,
            permit_id: self.permit_id,
            binding_digest: self.request.binding.digest(),
            admission_digest: self.request.admission_digest(),
            surface: self.request.surface,
            trace: self.trace.clone(),
        }
    }
    pub fn binding(&self) -> &ExecutionBinding {
        &self.request.binding
    }
    pub fn start(self) -> BrokeredExecution {
        BrokeredExecution {
            permit_id: self.permit_id,
            request: self.request,
            trace: self.trace,
            next_instruction: 0,
            usage: ResourceUsage::default(),
            verification_digest: None,
            buffered_visible: Vec::new(),
            staged_effects: Vec::new(),
        }
    }
}

pub fn prepare(request: PrepareRequest) -> Result<ExecutionPermit, PrepareFailure> {
    let mut trace = ExecutionTrace::new();
    macro_rules! guard {
        ($guard:expr, $check:expr) => {{
            if let Err(error) = $check {
                return Err(PrepareFailure { error, trace });
            }
            trace.pass($guard);
        }};
    }
    guard!(Guard::G0Canonical, validate_g0(&request));
    guard!(Guard::G1Coherence, validate_g1(&request));
    guard!(Guard::G2FinitePlan, validate_g2(&request));
    guard!(Guard::G3Attribution, validate_g3(&request));
    guard!(Guard::G4Resources, validate_g4(&request));
    guard!(Guard::G5RobustSnap, validate_g5(&request));
    guard!(Guard::G6SafetyShield, validate_g6(&request));
    guard!(Guard::G7Performance, validate_g7(&request));
    let permit_id = permit_digest(&request, &trace);
    Ok(ExecutionPermit {
        permit_id,
        request,
        trace,
    })
}

pub fn validate_permit_record(record: &PermitRecord) -> Result<(), KernelError> {
    if record.schema_version != TWO_PHASE_SCHEMA_VERSION {
        return Err(KernelError::execution(
            FailureCode::ForgedPermit,
            "permit schema version is not current",
        ));
    }
    record.trace.verify_prefix()?;
    if record.trace.events.len() != 8 {
        return Err(KernelError::execution(
            FailureCode::ForgedPermit,
            "permit trace must contain exactly G0-G7",
        ));
    }
    if is_zero(&record.binding_digest) || is_zero(&record.admission_digest) {
        return Err(KernelError::execution(
            FailureCode::ForgedPermit,
            "permit binding or admission digest is zero",
        ));
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&record.admission_digest);
    bytes.extend_from_slice(&record.trace.digest());
    if record.permit_id != hash_bytes(&bytes) {
        return Err(KernelError::execution(
            FailureCode::ForgedPermit,
            "permit identity does not bind its record",
        ));
    }
    Ok(())
}

fn validate_g0(request: &PrepareRequest) -> Result<(), KernelError> {
    if request.binding.schema_version != TWO_PHASE_SCHEMA_VERSION {
        return Err(KernelError::at(
            FailureCode::SchemaVersionMismatch,
            Guard::G0Canonical,
            "unsupported two-phase schema version",
        ));
    }
    if is_zero(&request.binding.assembly_manifest_digest)
        || is_zero(&request.binding.source_tree_digest)
        || is_zero(&request.binding.image_digest)
        || is_zero(&request.binding.comparison_identity_digest)
        || is_zero(&request.binding.plan_digest)
        || is_zero(&request.binding.predecessor_receipt_head)
    {
        return Err(KernelError::at(
            FailureCode::MissingBinding,
            Guard::G0Canonical,
            "required digest binding is zero",
        ));
    }
    if is_zero(&request.evidence.canonical_object_digest)
        || request.evidence.canonical_object_digest != request.evidence.decoded_object_digest
    {
        return Err(KernelError::at(
            FailureCode::CanonicalDigestMismatch,
            Guard::G0Canonical,
            "full-object digest differs from canonical decoded object",
        ));
    }
    Ok(())
}

fn validate_g1(request: &PrepareRequest) -> Result<(), KernelError> {
    if !(request.evidence.owner_coherent
        && request.evidence.producer_coherent
        && request.evidence.schema_coherent
        && request.evidence.source_root_coherent)
    {
        return Err(KernelError::at(
            FailureCode::CoherenceFailure,
            Guard::G1Coherence,
            "owner, producer, schema, and source root must all cohere",
        ));
    }
    validate_source_heads(&request.binding.source_repository_heads)
}

fn validate_g2(request: &PrepareRequest) -> Result<(), KernelError> {
    let instructions = &request.plan.instructions;
    if instructions.is_empty() || instructions.len() > MAX_CONTROLLER_INSTRUCTIONS {
        return Err(KernelError::at(
            FailureCode::InvalidPlan,
            Guard::G2FinitePlan,
            "plan length is zero or exceeds the frozen controller bound",
        ));
    }
    if !matches!(
        instructions.last(),
        Some(ControllerInstruction::CloseTransaction)
    ) || instructions[..instructions.len() - 1]
        .iter()
        .any(|step| matches!(step, ControllerInstruction::CloseTransaction))
    {
        return Err(KernelError::at(
            FailureCode::InvalidPlan,
            Guard::G2FinitePlan,
            "close_transaction must occur exactly once and last",
        ));
    }
    if !instructions
        .iter()
        .any(|step| matches!(step, ControllerInstruction::Dispatch { .. }))
    {
        return Err(KernelError::at(
            FailureCode::InvalidPlan,
            Guard::G2FinitePlan,
            "plan has no bounded worker dispatch",
        ));
    }
    if request.effect_class == EffectClass::ReadOnly
        && instructions
            .iter()
            .any(|step| matches!(step, ControllerInstruction::StageEffect))
    {
        return Err(KernelError::at(
            FailureCode::InvalidPlan,
            Guard::G2FinitePlan,
            "read-only plan stages an effect",
        ));
    }
    if request.effect_class != EffectClass::ReadOnly
        && !instructions
            .iter()
            .any(|step| matches!(step, ControllerInstruction::StageEffect))
    {
        return Err(KernelError::at(
            FailureCode::InvalidPlan,
            Guard::G2FinitePlan,
            "mutation plan contains no staged effect",
        ));
    }
    if request.plan.digest() != request.binding.plan_digest {
        return Err(KernelError::at(
            FailureCode::PlanDigestMismatch,
            Guard::G2FinitePlan,
            "controller plan does not match its bound digest",
        ));
    }
    Ok(())
}

fn validate_g3(request: &PrepareRequest) -> Result<(), KernelError> {
    if request.evidence.semantic_authority != SemanticAuthority::OwnerScoped {
        return Err(KernelError::at(
            FailureCode::SemanticCutCrossing,
            Guard::G3Attribution,
            "infrastructure may not choose task semantics",
        ));
    }
    if request.evidence.attribution_class != AttributionClass::Fixed {
        return Err(KernelError::at(
            FailureCode::AttributionChanged,
            Guard::G3Attribution,
            "comparison attribution class changed",
        ));
    }
    Ok(())
}

fn validate_g4(request: &PrepareRequest) -> Result<(), KernelError> {
    let envelope = request.envelope;
    if envelope.fuel == 0
        || envelope.deadline_ms == 0
        || envelope.io_bytes == 0
        || envelope.output_bytes == 0
        || envelope.memory_bytes == 0
        || envelope.processes == 0
        || envelope.risk_units == 0
        || envelope.worker_steps == 0
    {
        return Err(KernelError::at(
            FailureCode::UnboundedWorker,
            Guard::G4Resources,
            "every worker resource and risk bound must be nonzero",
        ));
    }
    Ok(())
}

fn validate_g5(request: &PrepareRequest) -> Result<(), KernelError> {
    match request.evidence.snap {
        SnapEvidence::NotClaimed => Ok(()),
        SnapEvidence::Verified { certificate_digest } if !is_zero(&certificate_digest) => Ok(()),
        SnapEvidence::Verified { .. } | SnapEvidence::ClaimedWithoutCertificate => {
            Err(KernelError::at(
                FailureCode::MissingSnapCertificate,
                Guard::G5RobustSnap,
                "S0 claim lacks a nonzero Robust Snap certificate",
            ))
        }
    }
}

fn validate_g6(request: &PrepareRequest) -> Result<(), KernelError> {
    if is_zero(&request.evidence.safety_shield_digest) {
        return Err(KernelError::at(
            FailureCode::MissingSafetyShield,
            Guard::G6SafetyShield,
            "V2 safety shield is absent",
        ));
    }
    if request.effect_class == EffectClass::ApprovalRequiredMutation
        && request
            .evidence
            .approval_grant_digest
            .is_none_or(|digest| is_zero(&digest))
    {
        return Err(KernelError::at(
            FailureCode::MissingApprovalGrant,
            Guard::G6SafetyShield,
            "approval-required execution lacks a validated grant commitment",
        ));
    }
    if request.effect_class == EffectClass::Irreversible
        && request
            .evidence
            .irreversible_pre_action_evidence_digest
            .map_or(true, |digest| is_zero(&digest))
    {
        return Err(KernelError::at(
            FailureCode::IrreversiblePreEvidenceEffect,
            Guard::G6SafetyShield,
            "irreversible execution requires verified pre-action evidence",
        ));
    }
    Ok(())
}

fn validate_g7(request: &PrepareRequest) -> Result<(), KernelError> {
    let valid = match request.evidence.performance {
        PerformanceAdmission::ExactNeutral | PerformanceAdmission::BaselineFallback => true,
        PerformanceAdmission::PointwiseDominance { evidence_digest }
        | PerformanceAdmission::ScopedCertificate { evidence_digest }
        | PerformanceAdmission::Distributional { evidence_digest } => !is_zero(&evidence_digest),
        PerformanceAdmission::Unknown => false,
    };
    if valid {
        Ok(())
    } else {
        Err(KernelError::at(
            FailureCode::PerformanceUnknown,
            Guard::G7Performance,
            "candidate performance is not admissible; select a proven baseline",
        ))
    }
}

fn validate_source_heads(heads: &[SourceHead]) -> Result<(), KernelError> {
    if heads.is_empty() || heads.len() > MAX_SOURCE_REPOSITORIES {
        return Err(KernelError::at(
            FailureCode::InvalidSourceIdentity,
            Guard::G1Coherence,
            "source head count is outside the frozen bound",
        ));
    }
    let mut previous: Option<(&str, &str)> = None;
    let mut unique = BTreeSet::new();
    for source in heads {
        let repository_valid = source.repository.len() > 0
            && source.repository.len() <= 64
            && source
                .repository
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte));
        let head_valid = (40..=64).contains(&source.head.len())
            && source
                .head
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
        if !repository_valid || !head_valid || !unique.insert((&source.repository, &source.head)) {
            return Err(KernelError::at(
                FailureCode::InvalidSourceIdentity,
                Guard::G1Coherence,
                "source heads must be bounded, canonical, and unique",
            ));
        }
        let current = (source.repository.as_str(), source.head.as_str());
        if previous.is_some_and(|prior| prior >= current) {
            return Err(KernelError::at(
                FailureCode::InvalidSourceIdentity,
                Guard::G1Coherence,
                "source heads must be strictly sorted",
            ));
        }
        previous = Some(current);
    }
    Ok(())
}

fn permit_digest(request: &PrepareRequest, trace: &ExecutionTrace) -> DigestV1 {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&request.admission_digest());
    bytes.extend_from_slice(&trace.digest());
    hash_bytes(&bytes)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StagedEffect {
    pub effect_digest: DigestV1,
    pub effect_class: EffectClass,
    pub approval_grant_digest: Option<DigestV1>,
    pub pre_action_evidence_digest: Option<DigestV1>,
}

/// Active execution owns private sinks. It exposes no output/effect getter.
#[derive(Debug)]
pub struct BrokeredExecution {
    permit_id: DigestV1,
    request: PrepareRequest,
    trace: ExecutionTrace,
    next_instruction: usize,
    usage: ResourceUsage,
    verification_digest: Option<DigestV1>,
    buffered_visible: Vec<u8>,
    staged_effects: Vec<StagedEffect>,
}

impl BrokeredExecution {
    pub fn dispatch(&mut self, owner: PeerOwner, usage: ResourceUsage) -> Result<(), KernelError> {
        self.expect(ControllerInstruction::Dispatch { owner })?;
        let next = self.usage.checked_add(usage).ok_or_else(|| {
            KernelError::execution(FailureCode::BoundExceeded, "resource counter overflow")
        })?;
        self.check_usage(next)?;
        self.usage = next;
        self.trace.worker_steps = next.worker_steps;
        self.advance();
        Ok(())
    }

    pub fn deterministic_transform(&mut self) -> Result<(), KernelError> {
        self.expect(ControllerInstruction::DeterministicTransform)?;
        self.advance();
        Ok(())
    }

    pub fn record_verification(&mut self, evidence_digest: DigestV1) -> Result<(), KernelError> {
        self.expect(ControllerInstruction::Verify)?;
        if is_zero(&evidence_digest) {
            return Err(KernelError::execution(
                FailureCode::IncompleteExecution,
                "verification evidence digest is zero",
            ));
        }
        self.verification_digest = Some(evidence_digest);
        self.advance();
        Ok(())
    }

    pub fn stage_effect(&mut self, effect: StagedEffect) -> Result<(), KernelError> {
        self.expect(ControllerInstruction::StageEffect)?;
        if effect.effect_class != self.request.effect_class || is_zero(&effect.effect_digest) {
            return Err(KernelError::execution(
                FailureCode::PlanStepMismatch,
                "staged effect does not match the admitted effect class",
            ));
        }
        if effect.effect_class == EffectClass::ApprovalRequiredMutation {
            let expected = self.request.evidence.approval_grant_digest;
            if expected.is_none() || effect.approval_grant_digest != expected {
                return Err(KernelError::execution(
                    FailureCode::MissingApprovalGrant,
                    "staged effect is not bound to the admitted approval grant",
                ));
            }
        }
        if effect.effect_class == EffectClass::Irreversible {
            let expected = self
                .request
                .evidence
                .irreversible_pre_action_evidence_digest;
            if expected.is_none() || effect.pre_action_evidence_digest != expected {
                return Err(KernelError::execution(
                    FailureCode::IrreversiblePreEvidenceEffect,
                    "irreversible staged effect is not bound to admitted pre-action evidence",
                ));
            }
        }
        self.staged_effects.push(effect);
        self.trace.staged_effects = self.staged_effects.len() as u32;
        self.advance();
        Ok(())
    }

    pub fn buffer_visible(&mut self, bytes: &[u8]) -> Result<(), KernelError> {
        self.expect(ControllerInstruction::BufferVisible)?;
        let new_len = self
            .buffered_visible
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| {
                KernelError::execution(
                    FailureCode::BufferOverflow,
                    "visible buffer length overflow",
                )
            })?;
        if new_len as u64 > self.request.envelope.output_bytes {
            return Err(KernelError::execution(
                FailureCode::BufferOverflow,
                "visible bytes exceed the admitted output bound",
            ));
        }
        self.buffered_visible.extend_from_slice(bytes);
        self.trace.buffered_visible_bytes = new_len as u64;
        self.advance();
        Ok(())
    }

    pub fn reject_early_publish(&self) -> KernelError {
        KernelError::execution(
            FailureCode::EarlyVisibleByte,
            "visible bytes remain private until G8/G9 finalize",
        )
    }

    pub fn close_transaction(
        mut self,
        closure: TransactionClosure,
    ) -> Result<ReadyToFinalize, KernelError> {
        self.expect(ControllerInstruction::CloseTransaction)?;
        self.advance();
        if self.next_instruction != self.request.plan.instructions.len()
            || self.verification_digest.is_none()
        {
            return Err(KernelError::execution(
                FailureCode::IncompleteExecution,
                "plan or evidence closure is incomplete",
            ));
        }
        self.into_ready(closure, None)
    }

    pub fn abort(
        mut self,
        failure: FailureCode,
        closure: TransactionClosure,
    ) -> Result<ReadyToFinalize, KernelError> {
        if closure.kind != ClosureKind::Fallback {
            return Err(KernelError::at(
                FailureCode::IncompleteTransactionClosure,
                Guard::G8TransactionClosure,
                "aborted execution requires fallback restoration",
            ));
        }
        self.trace.execution_failure = Some(failure);
        self.into_ready(closure, Some(failure))
    }

    fn into_ready(
        mut self,
        closure: TransactionClosure,
        failure: Option<FailureCode>,
    ) -> Result<ReadyToFinalize, KernelError> {
        validate_closure(&self, &closure, failure)?;
        self.trace.pass(Guard::G8TransactionClosure);
        Ok(ReadyToFinalize {
            permit_id: self.permit_id,
            request: self.request,
            trace: self.trace,
            usage: self.usage,
            verification_digest: self.verification_digest,
            buffered_visible: self.buffered_visible,
            staged_effects: self.staged_effects,
            closure,
        })
    }

    fn expect(&self, expected: ControllerInstruction) -> Result<(), KernelError> {
        match self.request.plan.instructions.get(self.next_instruction) {
            Some(actual) if *actual == expected => Ok(()),
            Some(actual) => Err(KernelError::execution(
                FailureCode::PlanStepMismatch,
                format!("expected {actual:?}, received {expected:?}"),
            )),
            None => Err(KernelError::execution(
                FailureCode::IncompleteExecution,
                "controller plan is exhausted",
            )),
        }
    }

    fn advance(&mut self) {
        self.next_instruction += 1;
        self.trace.executed_instructions = self.next_instruction as u32;
    }

    fn check_usage(&self, usage: ResourceUsage) -> Result<(), KernelError> {
        let envelope = self.request.envelope;
        let within = usage.fuel <= envelope.fuel
            && usage.elapsed_ms <= envelope.deadline_ms
            && usage.io_bytes <= envelope.io_bytes
            && usage.memory_bytes <= envelope.memory_bytes
            && usage.processes <= envelope.processes
            && usage.risk_units <= envelope.risk_units
            && usage.worker_steps <= envelope.worker_steps;
        if within {
            Ok(())
        } else {
            Err(KernelError::execution(
                FailureCode::BoundExceeded,
                "worker usage exceeds fuel/deadline/I/O/memory/process/risk/step bounds",
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosureKind {
    Commit,
    Fallback,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestorationAccounting {
    pub attempted: u64,
    pub completed: u64,
    pub debt: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionClosure {
    kind: ClosureKind,
    root: DigestV1,
    publication_closed: bool,
    restoration: RestorationAccounting,
}

impl TransactionClosure {
    pub fn commit(candidate_root: DigestV1, publication_closed: bool) -> Self {
        Self {
            kind: ClosureKind::Commit,
            root: candidate_root,
            publication_closed,
            restoration: RestorationAccounting {
                attempted: 0,
                completed: 0,
                debt: 0,
            },
        }
    }

    pub fn fallback(
        baseline_root: DigestV1,
        publication_closed: bool,
        restoration: RestorationAccounting,
    ) -> Self {
        Self {
            kind: ClosureKind::Fallback,
            root: baseline_root,
            publication_closed,
            restoration,
        }
    }

    pub fn kind(&self) -> ClosureKind {
        self.kind
    }
    pub fn root(&self) -> DigestV1 {
        self.root
    }
    pub fn restoration(&self) -> RestorationAccounting {
        self.restoration
    }
}

fn validate_closure(
    execution: &BrokeredExecution,
    closure: &TransactionClosure,
    failure: Option<FailureCode>,
) -> Result<(), KernelError> {
    if is_zero(&closure.root) || !closure.publication_closed {
        return Err(KernelError::at(
            FailureCode::IncompleteTransactionClosure,
            Guard::G8TransactionClosure,
            "transaction root is missing or publication boundary was not closed",
        ));
    }
    let accounted = closure
        .restoration
        .completed
        .checked_add(closure.restoration.debt);
    if accounted != Some(closure.restoration.attempted) {
        return Err(KernelError::at(
            FailureCode::UnaccountedFallback,
            Guard::G8TransactionClosure,
            "restoration attempted work is not conserved",
        ));
    }
    match closure.kind {
        ClosureKind::Commit => {
            if failure.is_some()
                || closure.restoration
                    != (RestorationAccounting {
                        attempted: 0,
                        completed: 0,
                        debt: 0,
                    })
            {
                return Err(KernelError::at(
                    FailureCode::IncompleteTransactionClosure,
                    Guard::G8TransactionClosure,
                    "commit closure contains failure or restoration work",
                ));
            }
            if matches!(
                execution.request.evidence.performance,
                PerformanceAdmission::BaselineFallback
            ) {
                return Err(KernelError::at(
                    FailureCode::PerformanceUnknown,
                    Guard::G8TransactionClosure,
                    "baseline-only admission cannot commit candidate output",
                ));
            }
        }
        ClosureKind::Fallback => {
            if closure.restoration.debt != 0
                || closure.restoration.completed != closure.restoration.attempted
            {
                return Err(KernelError::at(
                    FailureCode::UnaccountedFallback,
                    Guard::G8TransactionClosure,
                    "fallback restoration has residual debt",
                ));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ReceiptKind {
    Commit,
    Fallback,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptRecord {
    pub schema_version: u16,
    pub kind: ReceiptKind,
    pub permit_id: DigestV1,
    pub binding_digest: DigestV1,
    pub admission_digest: DigestV1,
    pub assembly_manifest_digest: DigestV1,
    pub source_tree_digest: DigestV1,
    pub source_repository_heads: Vec<SourceHead>,
    pub image_digest: DigestV1,
    pub plan_digest: DigestV1,
    pub comparison_identity_digest: DigestV1,
    pub surface: ExecutionSurface,
    pub verification_digest: Option<DigestV1>,
    pub output_digest: DigestV1,
    pub effects_digest: DigestV1,
    pub resource_usage: ResourceUsage,
    pub predecessor_receipt_head: DigestV1,
    pub successor_root: DigestV1,
    pub trace_digest: DigestV1,
    pub receipt_head: DigestV1,
    pub failure_code: Option<FailureCode>,
    pub restoration: RestorationAccounting,
}

/// Recomputes every public receipt commitment and rejects malformed chains.
pub fn validate_receipt_record(record: &ReceiptRecord) -> Result<(), KernelError> {
    if record.schema_version != TWO_PHASE_SCHEMA_VERSION {
        return Err(KernelError::execution(
            FailureCode::ForgedReceipt,
            "receipt schema version is not current",
        ));
    }
    validate_source_heads(&record.source_repository_heads)?;
    let binding = ExecutionBinding {
        schema_version: record.schema_version,
        assembly_manifest_digest: record.assembly_manifest_digest,
        source_tree_digest: record.source_tree_digest,
        source_repository_heads: record.source_repository_heads.clone(),
        image_digest: record.image_digest,
        plan_digest: record.plan_digest,
        comparison_identity_digest: record.comparison_identity_digest,
        predecessor_receipt_head: record.predecessor_receipt_head,
    };
    let required = [
        record.permit_id,
        record.binding_digest,
        record.admission_digest,
        record.assembly_manifest_digest,
        record.source_tree_digest,
        record.image_digest,
        record.plan_digest,
        record.comparison_identity_digest,
        record.output_digest,
        record.effects_digest,
        record.predecessor_receipt_head,
        record.successor_root,
        record.trace_digest,
        record.receipt_head,
    ];
    if required.iter().any(is_zero)
        || record
            .verification_digest
            .is_some_and(|digest| is_zero(&digest))
        || binding.digest() != record.binding_digest
    {
        return Err(KernelError::execution(
            FailureCode::ForgedReceipt,
            "receipt contains a zero, noncanonical, or mismatched binding",
        ));
    }
    let accounted = record
        .restoration
        .completed
        .checked_add(record.restoration.debt);
    let closure_valid = match record.kind {
        ReceiptKind::Commit => {
            record.failure_code.is_none()
                && record.restoration
                    == (RestorationAccounting {
                        attempted: 0,
                        completed: 0,
                        debt: 0,
                    })
        }
        ReceiptKind::Fallback => {
            record.restoration.debt == 0 && accounted == Some(record.restoration.attempted)
        }
    };
    if !closure_valid {
        return Err(KernelError::execution(
            FailureCode::ForgedReceipt,
            "receipt kind, failure, or restoration accounting is inconsistent",
        ));
    }
    let expected = receipt_digest(
        record.kind,
        record.permit_id,
        &binding,
        record.admission_digest,
        record.surface,
        record.verification_digest,
        record.output_digest,
        record.effects_digest,
        record.resource_usage,
        record.successor_root,
        record.trace_digest,
        record.failure_code,
        record.restoration,
    );
    if record.receipt_head != expected {
        return Err(KernelError::execution(
            FailureCode::ForgedReceipt,
            "receipt head does not match its canonical fields",
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub struct ReadyToFinalize {
    permit_id: DigestV1,
    request: PrepareRequest,
    trace: ExecutionTrace,
    usage: ResourceUsage,
    verification_digest: Option<DigestV1>,
    buffered_visible: Vec<u8>,
    staged_effects: Vec<StagedEffect>,
    closure: TransactionClosure,
}

impl ReadyToFinalize {
    pub fn finalize(mut self) -> Result<FinalReceipt, KernelError> {
        self.trace.pass(Guard::G9ReceiptCommitment);
        self.trace.verify_complete()?;
        let output_digest = hash_bytes(&self.buffered_visible);
        let effects_digest = effect_list_digest(&self.staged_effects);
        let kind = match self.closure.kind {
            ClosureKind::Commit => ReceiptKind::Commit,
            ClosureKind::Fallback => ReceiptKind::Fallback,
        };
        let admission_digest = self.request.admission_digest();
        let receipt_head = receipt_digest(
            kind,
            self.permit_id,
            &self.request.binding,
            admission_digest,
            self.request.surface,
            self.verification_digest,
            output_digest,
            effects_digest,
            self.usage,
            self.closure.root,
            self.trace.digest(),
            self.trace.execution_failure,
            self.closure.restoration,
        );
        let failure_code = self.trace.execution_failure;
        let common = ReceiptCommon {
            permit_id: self.permit_id,
            binding: self.request.binding,
            admission_digest,
            surface: self.request.surface,
            verification_digest: self.verification_digest,
            output_digest,
            effects_digest,
            usage: self.usage,
            successor_root: self.closure.root,
            trace: self.trace,
            receipt_head,
            failure_code,
            restoration: self.closure.restoration,
        };
        Ok(match kind {
            ReceiptKind::Commit => FinalReceipt::Commit(CommitReceipt {
                common,
                buffered_visible: self.buffered_visible,
                staged_effects: self.staged_effects,
            }),
            ReceiptKind::Fallback => FinalReceipt::Fallback(FallbackReceipt { common }),
        })
    }
}

#[derive(Debug)]
struct ReceiptCommon {
    permit_id: DigestV1,
    binding: ExecutionBinding,
    admission_digest: DigestV1,
    surface: ExecutionSurface,
    verification_digest: Option<DigestV1>,
    output_digest: DigestV1,
    effects_digest: DigestV1,
    usage: ResourceUsage,
    successor_root: DigestV1,
    trace: ExecutionTrace,
    receipt_head: DigestV1,
    failure_code: Option<FailureCode>,
    restoration: RestorationAccounting,
}

impl ReceiptCommon {
    fn record(&self, kind: ReceiptKind) -> ReceiptRecord {
        ReceiptRecord {
            schema_version: TWO_PHASE_SCHEMA_VERSION,
            kind,
            permit_id: self.permit_id,
            binding_digest: self.binding.digest(),
            admission_digest: self.admission_digest,
            assembly_manifest_digest: self.binding.assembly_manifest_digest,
            source_tree_digest: self.binding.source_tree_digest,
            source_repository_heads: self.binding.source_repository_heads.clone(),
            image_digest: self.binding.image_digest,
            plan_digest: self.binding.plan_digest,
            comparison_identity_digest: self.binding.comparison_identity_digest,
            surface: self.surface,
            verification_digest: self.verification_digest,
            output_digest: self.output_digest,
            effects_digest: self.effects_digest,
            resource_usage: self.usage,
            predecessor_receipt_head: self.binding.predecessor_receipt_head,
            successor_root: self.successor_root,
            trace_digest: self.trace.digest(),
            receipt_head: self.receipt_head,
            failure_code: self.failure_code,
            restoration: self.restoration,
        }
    }
}

#[derive(Debug)]
pub enum FinalReceipt {
    Commit(CommitReceipt),
    Fallback(FallbackReceipt),
}

/// Final candidate receipt. Publication consumes it and releases private sinks.
#[derive(Debug)]
pub struct CommitReceipt {
    common: ReceiptCommon,
    buffered_visible: Vec<u8>,
    staged_effects: Vec<StagedEffect>,
}
impl CommitReceipt {
    pub fn record(&self) -> ReceiptRecord {
        self.common.record(ReceiptKind::Commit)
    }
    pub fn trace(&self) -> &ExecutionTrace {
        &self.common.trace
    }
    pub fn publish(self) -> PublishedCommit {
        PublishedCommit {
            visible_bytes: self.buffered_visible,
            approved_effects: self.staged_effects,
            receipt_head: self.common.receipt_head,
            successor_root: self.common.successor_root,
        }
    }
}

/// Final fallback receipt. Candidate buffers and effects were dropped at G8/G9.
#[derive(Debug)]
pub struct FallbackReceipt {
    common: ReceiptCommon,
}
impl FallbackReceipt {
    pub fn record(&self) -> ReceiptRecord {
        self.common.record(ReceiptKind::Fallback)
    }
    pub fn trace(&self) -> &ExecutionTrace {
        &self.common.trace
    }
}

#[derive(Debug)]
pub struct PublishedCommit {
    pub visible_bytes: Vec<u8>,
    pub approved_effects: Vec<StagedEffect>,
    pub receipt_head: DigestV1,
    pub successor_root: DigestV1,
}

fn receipt_digest(
    kind: ReceiptKind,
    permit_id: DigestV1,
    binding: &ExecutionBinding,
    admission_digest: DigestV1,
    surface: ExecutionSurface,
    verification_digest: Option<DigestV1>,
    output_digest: DigestV1,
    effects_digest: DigestV1,
    usage: ResourceUsage,
    successor_root: DigestV1,
    trace_digest: DigestV1,
    failure: Option<FailureCode>,
    restoration: RestorationAccounting,
) -> DigestV1 {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&TWO_PHASE_SCHEMA_VERSION.to_be_bytes());
    bytes.push(kind as u8);
    bytes.extend_from_slice(&permit_id);
    bytes.extend_from_slice(&binding.digest());
    bytes.extend_from_slice(&admission_digest);
    bytes.push(surface as u8);
    bytes.extend_from_slice(&verification_digest.unwrap_or([0; 32]));
    bytes.extend_from_slice(&output_digest);
    bytes.extend_from_slice(&effects_digest);
    bytes.extend_from_slice(&usage.fuel.to_be_bytes());
    bytes.extend_from_slice(&usage.elapsed_ms.to_be_bytes());
    bytes.extend_from_slice(&usage.io_bytes.to_be_bytes());
    bytes.extend_from_slice(&usage.memory_bytes.to_be_bytes());
    bytes.extend_from_slice(&usage.processes.to_be_bytes());
    bytes.extend_from_slice(&usage.risk_units.to_be_bytes());
    bytes.extend_from_slice(&usage.worker_steps.to_be_bytes());
    bytes.extend_from_slice(&binding.predecessor_receipt_head);
    bytes.extend_from_slice(&successor_root);
    bytes.extend_from_slice(&trace_digest);
    bytes.push(failure.map_or(u8::MAX, |code| code as u8));
    bytes.extend_from_slice(&restoration.attempted.to_be_bytes());
    bytes.extend_from_slice(&restoration.completed.to_be_bytes());
    bytes.extend_from_slice(&restoration.debt.to_be_bytes());
    hash_bytes(&bytes)
}

fn effect_list_digest(effects: &[StagedEffect]) -> DigestV1 {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(effects.len() as u64).to_be_bytes());
    for effect in effects {
        bytes.extend_from_slice(&effect.effect_digest);
        bytes.push(match effect.effect_class {
            EffectClass::ReadOnly => 0,
            EffectClass::ReversibleMutation => 1,
            EffectClass::ApprovalRequiredMutation => 2,
            EffectClass::Irreversible => 3,
        });
        append_optional_digest(&mut bytes, effect.approval_grant_digest);
        append_optional_digest(&mut bytes, effect.pre_action_evidence_digest);
    }
    hash_bytes(&bytes)
}

fn append_bounded(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as u64).to_be_bytes());
    target.extend_from_slice(value);
}

fn append_optional_digest(target: &mut Vec<u8>, digest: Option<DigestV1>) {
    target.push(digest.is_some() as u8);
    if let Some(digest) = digest {
        target.extend_from_slice(&digest);
    }
}

fn hash_bytes(bytes: &[u8]) -> DigestV1 {
    Sha256::digest(bytes).into()
}
fn is_zero(digest: &DigestV1) -> bool {
    digest.iter().all(|byte| *byte == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> DigestV1 {
        [byte; 32]
    }

    fn plan(effect_class: EffectClass) -> ControllerPlan {
        let mut instructions = vec![
            ControllerInstruction::Dispatch {
                owner: PeerOwner::FsZero,
            },
            ControllerInstruction::DeterministicTransform,
            ControllerInstruction::Verify,
        ];
        if effect_class != EffectClass::ReadOnly {
            instructions.push(ControllerInstruction::StageEffect);
        }
        instructions.push(ControllerInstruction::BufferVisible);
        instructions.push(ControllerInstruction::CloseTransaction);
        ControllerPlan { instructions }
    }

    fn request(surface: ExecutionSurface, effect_class: EffectClass) -> PrepareRequest {
        let plan = plan(effect_class);
        PrepareRequest {
            binding: ExecutionBinding {
                schema_version: TWO_PHASE_SCHEMA_VERSION,
                assembly_manifest_digest: digest(1),
                source_tree_digest: digest(2),
                source_repository_heads: vec![SourceHead {
                    repository: "ZeroStack".into(),
                    head: "87c8ef5df0699b6345e4a829876b3f086f9c3ae5".into(),
                }],
                image_digest: digest(3),
                plan_digest: plan.digest(),
                comparison_identity_digest: digest(4),
                predecessor_receipt_head: digest(5),
            },
            surface,
            effect_class,
            plan,
            envelope: WorkerEnvelope {
                fuel: 100,
                deadline_ms: 100,
                io_bytes: 100,
                output_bytes: 32,
                memory_bytes: 1_024,
                processes: 1,
                risk_units: 10,
                worker_steps: 8,
            },
            evidence: GuardEvidence {
                canonical_object_digest: digest(6),
                decoded_object_digest: digest(6),
                owner_coherent: true,
                producer_coherent: true,
                schema_coherent: true,
                source_root_coherent: true,
                semantic_authority: SemanticAuthority::OwnerScoped,
                attribution_class: AttributionClass::Fixed,
                snap: SnapEvidence::NotClaimed,
                safety_shield_digest: digest(7),
                approval_grant_digest: (effect_class == EffectClass::ApprovalRequiredMutation)
                    .then(|| digest(12)),
                irreversible_pre_action_evidence_digest: if effect_class
                    == EffectClass::Irreversible
                {
                    Some(digest(8))
                } else {
                    None
                },
                performance: PerformanceAdmission::ExactNeutral,
            },
        }
    }

    fn run_to_ready(surface: ExecutionSurface) -> ReadyToFinalize {
        let permit = prepare(request(surface, EffectClass::ReversibleMutation)).unwrap();
        validate_permit_record(&permit.record()).unwrap();
        let mut execution = permit.start();
        execution
            .dispatch(
                PeerOwner::FsZero,
                ResourceUsage {
                    fuel: 10,
                    elapsed_ms: 4,
                    io_bytes: 8,
                    memory_bytes: 64,
                    processes: 1,
                    risk_units: 1,
                    worker_steps: 1,
                },
            )
            .unwrap();
        execution.deterministic_transform().unwrap();
        execution.record_verification(digest(9)).unwrap();
        execution
            .stage_effect(StagedEffect {
                effect_digest: digest(10),
                effect_class: EffectClass::ReversibleMutation,
                approval_grant_digest: None,
                pre_action_evidence_digest: None,
            })
            .unwrap();
        assert_eq!(
            execution.reject_early_publish().code,
            FailureCode::EarlyVisibleByte
        );
        execution.buffer_visible(b"accepted").unwrap();
        execution
            .close_transaction(TransactionClosure::commit(digest(11), true))
            .unwrap()
    }

    #[test]
    fn state_machine_prepare_execute_finalize_is_complete_and_linear() {
        let ready = run_to_ready(ExecutionSurface::Mcp);
        let FinalReceipt::Commit(receipt) = ready.finalize().unwrap() else {
            panic!("expected commit")
        };
        receipt.trace().verify_complete().unwrap();
        assert_eq!(
            receipt
                .trace()
                .events()
                .iter()
                .map(|event| event.guard)
                .collect::<Vec<_>>(),
            Guard::ALL
        );
        let record = receipt.record();
        assert_eq!(record.assembly_manifest_digest, digest(1));
        assert_eq!(record.predecessor_receipt_head, digest(5));
        assert_eq!(record.successor_root, digest(11));
        let published = receipt.publish();
        assert_eq!(published.visible_bytes, b"accepted");
        assert_eq!(published.approved_effects.len(), 1);
    }

    #[test]
    fn state_machine_all_surfaces_have_identical_guard_semantics() {
        for surface in [
            ExecutionSurface::Mcp,
            ExecutionSurface::Cli,
            ExecutionSurface::ClaudeCode,
            ExecutionSurface::Pi,
        ] {
            let FinalReceipt::Commit(receipt) = run_to_ready(surface).finalize().unwrap() else {
                panic!("expected commit")
            };
            assert_eq!(receipt.record().surface, surface);
            receipt.trace().verify_complete().unwrap();
        }
    }

    #[test]
    fn state_machine_omitted_guards_and_forged_predecessors_fail_typed() {
        let FinalReceipt::Commit(receipt) = run_to_ready(ExecutionSurface::Cli).finalize().unwrap()
        else {
            panic!("expected commit")
        };
        let complete = receipt.trace().clone();
        for index in 0..GUARD_COUNT {
            let mut mutant = complete.clone();
            mutant.events.remove(index);
            assert!(matches!(
                mutant.verify_complete().unwrap_err().code,
                FailureCode::IncompleteTrace | FailureCode::ForgedPredecessor
            ));
        }
        let mut mutant = complete;
        mutant.events[8].predecessor = Some(Guard::G6SafetyShield);
        assert_eq!(
            mutant.verify_complete().unwrap_err().code,
            FailureCode::ForgedPredecessor
        );
    }

    #[test]
    fn state_machine_forged_permit_unbounded_worker_and_semantic_cut_fail() {
        let permit = prepare(request(ExecutionSurface::Pi, EffectClass::ReadOnly)).unwrap();
        let mut record = permit.record();
        record.permit_id[0] ^= 1;
        assert_eq!(
            validate_permit_record(&record).unwrap_err().code,
            FailureCode::ForgedPermit
        );
        let mut unbounded = request(ExecutionSurface::Pi, EffectClass::ReadOnly);
        unbounded.envelope.fuel = 0;
        assert_eq!(
            prepare(unbounded).unwrap_err().error().code,
            FailureCode::UnboundedWorker
        );
        let mut cut = request(ExecutionSurface::Pi, EffectClass::ReadOnly);
        cut.evidence.semantic_authority = SemanticAuthority::HiddenTaskSelector;
        assert_eq!(
            prepare(cut).unwrap_err().error().code,
            FailureCode::SemanticCutCrossing
        );
    }

    #[test]
    fn state_machine_buffer_overflow_falls_back_only_after_full_restoration() {
        let permit = prepare(request(ExecutionSurface::ClaudeCode, EffectClass::ReadOnly)).unwrap();
        let mut execution = permit.start();
        execution
            .dispatch(
                PeerOwner::FsZero,
                ResourceUsage {
                    worker_steps: 1,
                    ..ResourceUsage::default()
                },
            )
            .unwrap();
        execution.deterministic_transform().unwrap();
        execution.record_verification(digest(9)).unwrap();
        let error = execution.buffer_visible(&[0; 33]).unwrap_err();
        assert_eq!(error.code, FailureCode::BufferOverflow);
        let bad = TransactionClosure::fallback(
            digest(12),
            true,
            RestorationAccounting {
                attempted: 2,
                completed: 1,
                debt: 0,
            },
        );
        assert_eq!(
            execution.abort(error.code, bad).unwrap_err().code,
            FailureCode::UnaccountedFallback
        );

        let permit = prepare(request(ExecutionSurface::ClaudeCode, EffectClass::ReadOnly)).unwrap();
        let execution = permit.start();
        let ready = execution
            .abort(
                FailureCode::BufferOverflow,
                TransactionClosure::fallback(
                    digest(12),
                    true,
                    RestorationAccounting {
                        attempted: 2,
                        completed: 2,
                        debt: 0,
                    },
                ),
            )
            .unwrap();
        let FinalReceipt::Fallback(receipt) = ready.finalize().unwrap() else {
            panic!("expected fallback")
        };
        receipt.trace().verify_complete().unwrap();
        assert_eq!(
            receipt.record().failure_code,
            Some(FailureCode::BufferOverflow)
        );
        assert_eq!(receipt.record().restoration.completed, 2);
    }

    #[test]
    fn state_machine_irreversible_effect_requires_matching_pre_action_evidence() {
        let permit = prepare(request(ExecutionSurface::Mcp, EffectClass::Irreversible)).unwrap();
        let mut execution = permit.start();
        execution
            .dispatch(
                PeerOwner::FsZero,
                ResourceUsage {
                    worker_steps: 1,
                    ..ResourceUsage::default()
                },
            )
            .unwrap();
        execution.deterministic_transform().unwrap();
        execution.record_verification(digest(9)).unwrap();
        let error = execution
            .stage_effect(StagedEffect {
                effect_digest: digest(10),
                effect_class: EffectClass::Irreversible,
                approval_grant_digest: None,
                pre_action_evidence_digest: Some(digest(99)),
            })
            .unwrap_err();
        assert_eq!(error.code, FailureCode::IrreversiblePreEvidenceEffect);
    }

    #[test]
    fn state_machine_admission_and_receipt_commitments_reject_tampering() {
        let mut missing_predecessor = request(ExecutionSurface::Mcp, EffectClass::ReadOnly);
        missing_predecessor.binding.predecessor_receipt_head = [0; 32];
        assert_eq!(
            prepare(missing_predecessor).unwrap_err().error().code,
            FailureCode::MissingBinding
        );

        let base = request(ExecutionSurface::Mcp, EffectClass::ReversibleMutation);
        let base_digest = base.admission_digest();
        let mut changed_envelope = base.clone();
        changed_envelope.envelope.fuel += 1;
        assert_ne!(base_digest, changed_envelope.admission_digest());
        let mut changed_evidence = base.clone();
        changed_evidence.evidence.safety_shield_digest = digest(99);
        assert_ne!(base_digest, changed_evidence.admission_digest());

        let permit = prepare(base).unwrap();
        let mut permit_record = permit.record();
        validate_permit_record(&permit_record).unwrap();
        permit_record.admission_digest[0] ^= 1;
        assert_eq!(
            validate_permit_record(&permit_record).unwrap_err().code,
            FailureCode::ForgedPermit
        );

        let FinalReceipt::Commit(receipt) = run_to_ready(ExecutionSurface::Mcp).finalize().unwrap()
        else {
            panic!("expected commit")
        };
        let mut receipt_record = receipt.record();
        validate_receipt_record(&receipt_record).unwrap();
        receipt_record.output_digest[0] ^= 1;
        assert_eq!(
            validate_receipt_record(&receipt_record).unwrap_err().code,
            FailureCode::ForgedReceipt
        );
    }

    #[test]
    fn state_machine_approval_required_effect_binds_validated_grant() {
        let mut missing = request(ExecutionSurface::Mcp, EffectClass::ApprovalRequiredMutation);
        missing.evidence.approval_grant_digest = None;
        assert_eq!(
            prepare(missing).unwrap_err().error().code,
            FailureCode::MissingApprovalGrant
        );

        let permit = prepare(request(
            ExecutionSurface::Mcp,
            EffectClass::ApprovalRequiredMutation,
        ))
        .unwrap();
        let mut execution = permit.start();
        execution
            .dispatch(
                PeerOwner::FsZero,
                ResourceUsage {
                    worker_steps: 1,
                    ..ResourceUsage::default()
                },
            )
            .unwrap();
        execution.deterministic_transform().unwrap();
        execution.record_verification(digest(9)).unwrap();
        let error = execution
            .stage_effect(StagedEffect {
                effect_digest: digest(10),
                effect_class: EffectClass::ApprovalRequiredMutation,
                approval_grant_digest: Some(digest(99)),
                pre_action_evidence_digest: None,
            })
            .unwrap_err();
        assert_eq!(error.code, FailureCode::MissingApprovalGrant);
        execution
            .stage_effect(StagedEffect {
                effect_digest: digest(10),
                effect_class: EffectClass::ApprovalRequiredMutation,
                approval_grant_digest: Some(digest(12)),
                pre_action_evidence_digest: None,
            })
            .unwrap();
    }
}
