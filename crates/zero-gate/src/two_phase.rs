//! Assembly-bound two-phase execution kernel.
//!
//! `ExecutionPermit`, `BrokeredExecution`, `ReadyToFinalize`, and the final
//! receipts are linear capabilities. Their fields are private, they are not
//! cloneable, and only the preceding phase can construct the next phase.

use crate::quality::{
    quality_envelope_contract_digest_v1, QualityAdmissionRecordV1, QualityAdmissionV1,
    QualityEvidenceClassV1, QualityGuaranteeV1, QualitySelectionV1,
};
use crate::transaction::{
    transaction_contract_digest_v1, RestorationScopeV1, TransactionDispositionV1,
    TransactionReceiptV1,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use zero_abi::{
    canonical_json, raw_worker::EffectClass, zbf_contract_digest_v1, ArtifactOwnerV1,
    DigestV1 as AbiDigestV1, DurableProfileV1, RobustSnapCertificate, SnapLevel, ZbfArtifactKindV1,
    ZbfObjectV1,
};
use zero_cert::{effect_witness_contract_digest_v1, EffectAcceptedV1, VerifiedEvidence};

pub const TWO_PHASE_SCHEMA_VERSION: u16 = 3;
pub const GUARD_COUNT: usize = 10;
pub const MAX_SOURCE_REPOSITORIES: usize = 64;
pub const MAX_CONTROLLER_INSTRUCTIONS: usize = 4_096;

pub type DigestV1 = [u8; 32];

const TWO_PHASE_CONTRACT_DOMAIN_V2: &[u8] = b"zerostack.kernel.contract.v2\0";
const TWO_PHASE_CONTRACT_DOMAIN_V3: &[u8] = b"zerostack.kernel.contract.v3\0";

pub fn two_phase_contract_manifest_v2() -> Value {
    json!({
        "artifact_profile": "zbf_1_portable_strict",
        "contract_version": 2,
        "guard_order": Guard::ALL,
        "linked_contracts": {
            "effect_witness": effect_witness_contract_digest_v1(),
            "transaction": transaction_contract_digest_v1(),
            "zbf": zbf_contract_digest_v1(),
        },
        "name": "zerostack.two_phase_kernel.v2",
        "negative_space": [
            "native_filesystem_durability",
            "production_worker_contract_enforcement",
            "quality_modes_without_proof",
            "universal_external_state_restoration",
        ],
        "quality_modes_admitted": ["exact_neutral", "baseline_fallback"],
        "receipt_bindings": [
            "schema_version",
            "kind",
            "permit_id",
            "binding_digest",
            "admission_digest",
            "assembly_manifest_digest",
            "source_tree_digest",
            "source_repository_heads",
            "image_digest",
            "state_snapshot_digest",
            "task_fingerprint_digest",
            "plan_digest",
            "fixed_model_digest",
            "comparison_identity_digest",
            "artifact_set_digest",
            "semantic_cut_certificate_digest",
            "snap_certificate_digest",
            "safety_shield_digest",
            "quality_decision_digest",
            "transaction_receipt_digest",
            "attribution_class",
            "effect_class",
            "resource_envelope",
            "surface",
            "verification_digest",
            "output_digest",
            "effects_digest",
            "resource_usage",
            "predecessor_receipt_head",
            "successor_root",
            "trace_digest",
            "failure_code",
            "restoration",
            "receipt_head",
        ],
        "transaction_closure": "validated_zero_gate_transaction_receipt_only",
    })
}

pub fn two_phase_contract_digest_v2() -> DigestV1 {
    let canonical = canonical_json(&two_phase_contract_manifest_v2());
    let mut bytes = Vec::with_capacity(TWO_PHASE_CONTRACT_DOMAIN_V2.len() + canonical.len());
    bytes.extend_from_slice(TWO_PHASE_CONTRACT_DOMAIN_V2);
    bytes.extend_from_slice(canonical.as_bytes());
    hash_bytes(&bytes)
}

pub fn two_phase_contract_manifest_v3() -> Value {
    json!({
        "artifact_profile": "zbf_1_portable_strict",
        "candidate_protocol_identity": [
            "assembly_manifest_digest",
            "source_tree_digest",
            "image_digest",
            "fixed_model_digest",
            "two_phase_contract_digest_v3",
        ],
        "contract_version": TWO_PHASE_SCHEMA_VERSION,
        "guard_order": Guard::ALL,
        "linked_contracts": {
            "effect_witness": effect_witness_contract_digest_v1(),
            "quality_envelope": quality_envelope_contract_digest_v1(),
            "transaction": transaction_contract_digest_v1(),
            "zbf": zbf_contract_digest_v1(),
        },
        "name": "zerostack.two_phase_kernel.v3",
        "negative_space": [
            "individual_candidate_claim_from_distributional_evidence",
            "native_filesystem_durability",
            "production_worker_contract_enforcement",
            "universal_external_state_restoration",
        ],
        "quality_modes_admitted": [
            "exact_neutral",
            "pointwise_dominance",
            "scoped_class_dominance",
            "distributional_baseline_only",
            "unidentified_baseline",
        ],
        "receipt_bindings": [
            "schema_version",
            "kind",
            "permit_id",
            "binding_digest",
            "admission_digest",
            "assembly_manifest_digest",
            "source_tree_digest",
            "source_repository_heads",
            "image_digest",
            "state_snapshot_digest",
            "task_fingerprint_digest",
            "plan_digest",
            "fixed_model_digest",
            "comparison_identity_digest",
            "artifact_set_digest",
            "semantic_cut_certificate_digest",
            "snap_certificate_digest",
            "safety_shield_digest",
            "quality_admission",
            "final_quality_selection",
            "transaction_receipt_digest",
            "attribution_class",
            "effect_class",
            "resource_envelope",
            "surface",
            "verification_digest",
            "output_digest",
            "effects_digest",
            "resource_usage",
            "predecessor_receipt_head",
            "successor_root",
            "trace_digest",
            "failure_code",
            "restoration",
            "receipt_head",
        ],
        "transaction_closure": "validated_zero_gate_transaction_receipt_only",
    })
}

pub fn two_phase_contract_digest_v3() -> DigestV1 {
    let canonical = canonical_json(&two_phase_contract_manifest_v3());
    let mut bytes = Vec::with_capacity(TWO_PHASE_CONTRACT_DOMAIN_V3.len() + canonical.len());
    bytes.extend_from_slice(TWO_PHASE_CONTRACT_DOMAIN_V3);
    bytes.extend_from_slice(canonical.as_bytes());
    hash_bytes(&bytes)
}

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

#[derive(Clone, Debug)]
pub struct PeerArtifactInputV1 {
    pub bytes: Vec<u8>,
    pub expected_owner: ArtifactOwnerV1,
    pub expected_kind: ZbfArtifactKindV1,
    pub expected_producer_contract_digest: DigestV1,
}

/// Opaque G0/G1 evidence minted only by strict ZBF decode and coherence checks.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalArtifactSetV1 {
    assembly_manifest_digest: DigestV1,
    source_root_digest: DigestV1,
    image_digest: DigestV1,
    artifact_set_digest: DigestV1,
    artifact_identities: Vec<DigestV1>,
    producer_contract_digests: Vec<DigestV1>,
}

impl CanonicalArtifactSetV1 {
    pub fn verify(
        assembly_manifest_digest: DigestV1,
        source_root_digest: DigestV1,
        artifacts: Vec<PeerArtifactInputV1>,
    ) -> Result<Self, KernelError> {
        if is_zero(&assembly_manifest_digest) || is_zero(&source_root_digest) {
            return Err(KernelError::at(
                FailureCode::MissingBinding,
                Guard::G0Canonical,
                "artifact assembly and source-root bindings must be nonzero",
            ));
        }
        let expected = [
            (ArtifactOwnerV1::FsZero, ZbfArtifactKindV1::FsPack),
            (ArtifactOwnerV1::GraphZero, ZbfArtifactKindV1::GraphPack),
            (ArtifactOwnerV1::TokenZero, ZbfArtifactKindV1::TokenPack),
        ];
        if artifacts.len() != expected.len() {
            return Err(KernelError::at(
                FailureCode::InvalidSourceIdentity,
                Guard::G1Coherence,
                "canonical Zero Image requires exactly one FS, graph, and token pack",
            ));
        }
        let assembly = AbiDigestV1::from_bytes(assembly_manifest_digest);
        let source_root = AbiDigestV1::from_bytes(source_root_digest);
        let profile = DurableProfileV1::portable_strict();
        let mut artifact_identities = Vec::with_capacity(expected.len());
        let mut producer_contract_digests = Vec::with_capacity(expected.len());
        for (index, (input, (owner, kind))) in artifacts.into_iter().zip(expected).enumerate() {
            if input.expected_owner != owner || input.expected_kind != kind {
                return Err(KernelError::at(
                    FailureCode::InvalidSourceIdentity,
                    Guard::G1Coherence,
                    format!("peer artifact {index} is not in canonical FS/graph/token order"),
                ));
            }
            if is_zero(&input.expected_producer_contract_digest) {
                return Err(KernelError::at(
                    FailureCode::MissingBinding,
                    Guard::G1Coherence,
                    format!("peer artifact {index} has a zero producer contract"),
                ));
            }
            let object =
                ZbfObjectV1::from_bytes(&input.bytes, assembly, profile).map_err(|error| {
                    KernelError::at(
                        FailureCode::CanonicalDigestMismatch,
                        Guard::G0Canonical,
                        format!("peer artifact {index} failed strict ZBF decode: {error}"),
                    )
                })?;
            if object.header.owner != owner
                || object.header.kind != kind
                || object.header.source_root_digest != source_root
                || object.header.producer_contract_digest
                    != AbiDigestV1::from_bytes(input.expected_producer_contract_digest)
            {
                return Err(KernelError::at(
                    FailureCode::CoherenceFailure,
                    Guard::G1Coherence,
                    format!("peer artifact {index} owner/kind/source/producer binding differs"),
                ));
            }
            let identity = object.identity(profile).map_err(|error| {
                KernelError::at(
                    FailureCode::CanonicalDigestMismatch,
                    Guard::G0Canonical,
                    format!("peer artifact {index} identity failed: {error}"),
                )
            })?;
            artifact_identities.push(*identity.as_bytes());
            producer_contract_digests.push(input.expected_producer_contract_digest);
        }
        let image_digest = image_digest_v1(source_root_digest, &artifact_identities);
        let mut commitment = Vec::new();
        commitment.extend_from_slice(b"zerostack.kernel.artifact_set.v2\0");
        commitment.extend_from_slice(&assembly_manifest_digest);
        commitment.extend_from_slice(&source_root_digest);
        commitment.extend_from_slice(&image_digest);
        for (identity, producer) in artifact_identities.iter().zip(&producer_contract_digests) {
            commitment.extend_from_slice(identity);
            commitment.extend_from_slice(producer);
        }
        Ok(Self {
            assembly_manifest_digest,
            source_root_digest,
            image_digest,
            artifact_set_digest: hash_bytes(&commitment),
            artifact_identities,
            producer_contract_digests,
        })
    }

    pub const fn image_digest(&self) -> DigestV1 {
        self.image_digest
    }

    pub const fn artifact_set_digest(&self) -> DigestV1 {
        self.artifact_set_digest
    }
}

fn image_digest_v1(source_root: DigestV1, artifacts: &[DigestV1]) -> DigestV1 {
    let mut bytes = Vec::with_capacity(64 + artifacts.len() * 32);
    bytes.extend_from_slice(b"zerostack.kernel.image.v2\0");
    bytes.extend_from_slice(&source_root);
    bytes.extend_from_slice(&(artifacts.len() as u64).to_be_bytes());
    for artifact in artifacts {
        bytes.extend_from_slice(artifact);
    }
    hash_bytes(&bytes)
}

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
        let mut bytes = Vec::with_capacity(96 + self.events.len() * 3);
        bytes.extend_from_slice(b"zerostack.kernel.trace.v3\0");
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
    pub state_snapshot_digest: DigestV1,
    pub task_fingerprint_digest: DigestV1,
    pub plan_digest: DigestV1,
    pub fixed_model_digest: DigestV1,
    pub comparison_identity_digest: DigestV1,
    pub predecessor_receipt_head: DigestV1,
}

impl ExecutionBinding {
    pub fn digest(&self) -> DigestV1 {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"zerostack.kernel.binding.v3\0");
        bytes.extend_from_slice(&self.schema_version.to_be_bytes());
        bytes.extend_from_slice(&self.assembly_manifest_digest);
        bytes.extend_from_slice(&self.source_tree_digest);
        bytes.extend_from_slice(&(self.source_repository_heads.len() as u64).to_be_bytes());
        for source in &self.source_repository_heads {
            append_bounded(&mut bytes, source.repository.as_bytes());
            append_bounded(&mut bytes, source.head.as_bytes());
        }
        bytes.extend_from_slice(&self.image_digest);
        bytes.extend_from_slice(&self.state_snapshot_digest);
        bytes.extend_from_slice(&self.task_fingerprint_digest);
        bytes.extend_from_slice(&self.plan_digest);
        bytes.extend_from_slice(&self.fixed_model_digest);
        bytes.extend_from_slice(&self.comparison_identity_digest);
        bytes.extend_from_slice(&self.predecessor_receipt_head);
        hash_bytes(&bytes)
    }
}

pub fn candidate_protocol_identity_v1(binding: &ExecutionBinding) -> DigestV1 {
    let mut bytes = Vec::with_capacity(32 * 5);
    bytes.extend_from_slice(&binding.assembly_manifest_digest);
    bytes.extend_from_slice(&binding.source_tree_digest);
    bytes.extend_from_slice(&binding.image_digest);
    bytes.extend_from_slice(&binding.fixed_model_digest);
    bytes.extend_from_slice(&two_phase_contract_digest_v3());
    let mut framed = b"ZERO.TWO_PHASE.CANDIDATE_PROTOCOL.V1\0".to_vec();
    framed.extend_from_slice(&bytes);
    hash_bytes(&framed)
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
        let mut bytes = Vec::with_capacity(40 + self.instructions.len());
        bytes.extend_from_slice(b"zerostack.kernel.plan.v3\0");
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

/// Opaque G3 evidence binding the semantic cut to the frozen comparison identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticCutEvidenceV1 {
    certificate_digest: DigestV1,
    plan_digest: DigestV1,
    fixed_model_digest: DigestV1,
    comparison_identity_digest: DigestV1,
    semantic_authority: SemanticAuthority,
    attribution_class: AttributionClass,
}

impl SemanticCutEvidenceV1 {
    pub fn verify_owner_scoped(
        plan_digest: DigestV1,
        fixed_model_digest: DigestV1,
        comparison_identity_digest: DigestV1,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, KernelError> {
        if [plan_digest, fixed_model_digest, comparison_identity_digest]
            .iter()
            .any(is_zero)
        {
            return Err(KernelError::at(
                FailureCode::SemanticCutCrossing,
                Guard::G3Attribution,
                "semantic-cut identity bindings must be nonzero",
            ));
        }
        let certificate_digest = evidence.certificate().canonical_digest().map_err(|error| {
            KernelError::at(
                FailureCode::SemanticCutCrossing,
                Guard::G3Attribution,
                format!("semantic-cut evidence is not canonical: {error}"),
            )
        })?;
        Ok(Self {
            certificate_digest,
            plan_digest,
            fixed_model_digest,
            comparison_identity_digest,
            semantic_authority: SemanticAuthority::OwnerScoped,
            attribution_class: AttributionClass::Fixed,
        })
    }

    pub const fn certificate_digest(&self) -> DigestV1 {
        self.certificate_digest
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum SnapEvidence {
    NotClaimed,
    Verified { certificate: RobustSnapCertificate },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SafetyShieldKindV1 {
    ReadOnly,
    AcceptedEffect,
}

/// Opaque G6 evidence minted from verified zero-cert evidence or EffectAcceptedV1.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SafetyShieldEvidenceV1 {
    kind: SafetyShieldKindV1,
    shield_digest: DigestV1,
    state_snapshot: DigestV1,
    action_digest: Option<DigestV1>,
    evidence_digest: DigestV1,
    verifier_digest: DigestV1,
    acceptance_digest: Option<DigestV1>,
    accepted_effect: Option<EffectAcceptedV1>,
}

impl SafetyShieldEvidenceV1 {
    pub fn from_read_only_verified(
        state_snapshot: DigestV1,
        verifier_digest: DigestV1,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, KernelError> {
        if is_zero(&state_snapshot) || is_zero(&verifier_digest) {
            return Err(KernelError::at(
                FailureCode::MissingSafetyShield,
                Guard::G6SafetyShield,
                "read-only shield state and verifier bindings must be nonzero",
            ));
        }
        let evidence_digest = evidence.certificate().canonical_digest().map_err(|error| {
            KernelError::at(
                FailureCode::MissingSafetyShield,
                Guard::G6SafetyShield,
                format!("verified read-only evidence is not canonical: {error}"),
            )
        })?;
        let shield_digest = safety_shield_digest_v1(
            SafetyShieldKindV1::ReadOnly,
            state_snapshot,
            None,
            evidence_digest,
            verifier_digest,
            None,
        );
        Ok(Self {
            kind: SafetyShieldKindV1::ReadOnly,
            shield_digest,
            state_snapshot,
            action_digest: None,
            evidence_digest,
            verifier_digest,
            acceptance_digest: None,
            accepted_effect: None,
        })
    }

    pub fn from_effect_accepted(accepted: EffectAcceptedV1) -> Result<Self, KernelError> {
        accepted.validate().map_err(|error| {
            KernelError::at(
                FailureCode::MissingSafetyShield,
                Guard::G6SafetyShield,
                format!("effect acceptance is invalid: {error}"),
            )
        })?;
        let state_snapshot = *accepted.state_snapshot().as_bytes();
        let action_digest = *accepted.action_digest().as_bytes();
        let evidence_digest = *accepted.evidence_digest().as_bytes();
        let verifier_digest = *accepted.verifier_digest().as_bytes();
        let acceptance_digest = *accepted.acceptance_digest().as_bytes();
        let shield_digest = safety_shield_digest_v1(
            SafetyShieldKindV1::AcceptedEffect,
            state_snapshot,
            Some(action_digest),
            evidence_digest,
            verifier_digest,
            Some(acceptance_digest),
        );
        Ok(Self {
            kind: SafetyShieldKindV1::AcceptedEffect,
            shield_digest,
            state_snapshot,
            action_digest: Some(action_digest),
            evidence_digest,
            verifier_digest,
            acceptance_digest: Some(acceptance_digest),
            accepted_effect: Some(accepted),
        })
    }

    pub const fn shield_digest(&self) -> DigestV1 {
        self.shield_digest
    }

    pub const fn action_digest(&self) -> Option<DigestV1> {
        self.action_digest
    }

    pub const fn acceptance_digest(&self) -> Option<DigestV1> {
        self.acceptance_digest
    }
}

fn safety_shield_digest_v1(
    kind: SafetyShieldKindV1,
    state_snapshot: DigestV1,
    action_digest: Option<DigestV1>,
    evidence_digest: DigestV1,
    verifier_digest: DigestV1,
    acceptance_digest: Option<DigestV1>,
) -> DigestV1 {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"zerostack.kernel.safety_shield.v2\0");
    bytes.push(match kind {
        SafetyShieldKindV1::ReadOnly => 0,
        SafetyShieldKindV1::AcceptedEffect => 1,
    });
    bytes.extend_from_slice(&state_snapshot);
    append_optional_digest(&mut bytes, action_digest);
    bytes.extend_from_slice(&evidence_digest);
    bytes.extend_from_slice(&verifier_digest);
    append_optional_digest(&mut bytes, acceptance_digest);
    hash_bytes(&bytes)
}

/// Backward-compatible kernel name for the proof-carrying quality decision.
pub type PerformanceAdmission = QualityAdmissionV1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuardEvidence {
    pub artifacts: CanonicalArtifactSetV1,
    pub semantic_cut: SemanticCutEvidenceV1,
    pub snap: SnapEvidence,
    pub safety_shield: SafetyShieldEvidenceV1,
    pub approval_grant_digest: Option<DigestV1>,
    pub irreversible_pre_action_evidence_digest: Option<DigestV1>,
    pub performance: PerformanceAdmission,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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
        bytes.extend_from_slice(b"zerostack.kernel.admission.v3\0");
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
        bytes.extend_from_slice(&evidence.artifacts.artifact_set_digest);
        bytes.extend_from_slice(&evidence.semantic_cut.certificate_digest);
        bytes.push(match evidence.semantic_cut.semantic_authority {
            SemanticAuthority::OwnerScoped => 0,
            SemanticAuthority::HiddenTaskSelector => 1,
        });
        bytes.push(match evidence.semantic_cut.attribution_class {
            AttributionClass::Fixed => 0,
            AttributionClass::Changed => 1,
        });
        match &evidence.snap {
            SnapEvidence::NotClaimed => bytes.push(0),
            SnapEvidence::Verified { certificate } => {
                bytes.push(1);
                bytes.extend_from_slice(certificate.certificate_digest.as_bytes());
            }
        }
        bytes.extend_from_slice(&evidence.safety_shield.shield_digest);
        append_optional_digest(&mut bytes, evidence.approval_grant_digest);
        append_optional_digest(&mut bytes, evidence.irreversible_pre_action_evidence_digest);
        bytes.extend_from_slice(evidence.performance.digest().as_bytes());
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
    bytes.extend_from_slice(b"zerostack.kernel.permit.v3\0");
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
            "unsupported trusted-kernel schema version",
        ));
    }
    if [
        request.binding.assembly_manifest_digest,
        request.binding.source_tree_digest,
        request.binding.image_digest,
        request.binding.state_snapshot_digest,
        request.binding.task_fingerprint_digest,
        request.binding.plan_digest,
        request.binding.fixed_model_digest,
        request.binding.comparison_identity_digest,
        request.binding.predecessor_receipt_head,
        request.evidence.artifacts.artifact_set_digest,
    ]
    .iter()
    .any(is_zero)
    {
        return Err(KernelError::at(
            FailureCode::MissingBinding,
            Guard::G0Canonical,
            "required canonical artifact, state, task, model, or receipt binding is zero",
        ));
    }
    if request.evidence.artifacts.artifact_identities.len() != 3
        || request.evidence.artifacts.producer_contract_digests.len() != 3
    {
        return Err(KernelError::at(
            FailureCode::CanonicalDigestMismatch,
            Guard::G0Canonical,
            "verified artifact set no longer contains exactly three peer packs",
        ));
    }
    Ok(())
}

fn validate_g1(request: &PrepareRequest) -> Result<(), KernelError> {
    let artifacts = &request.evidence.artifacts;
    if artifacts.assembly_manifest_digest != request.binding.assembly_manifest_digest
        || artifacts.source_root_digest != request.binding.source_tree_digest
        || artifacts.image_digest != request.binding.image_digest
        || artifacts.image_digest
            != image_digest_v1(artifacts.source_root_digest, &artifacts.artifact_identities)
        || artifacts.producer_contract_digests.iter().any(is_zero)
    {
        return Err(KernelError::at(
            FailureCode::CoherenceFailure,
            Guard::G1Coherence,
            "peer assembly, producer, source-root, or recomputed image identity differs",
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
    let count = |needle: fn(&ControllerInstruction) -> bool| {
        instructions.iter().filter(|step| needle(step)).count()
    };
    if !matches!(
        instructions.last(),
        Some(ControllerInstruction::CloseTransaction)
    ) || count(|step| matches!(step, ControllerInstruction::CloseTransaction)) != 1
        || count(|step| matches!(step, ControllerInstruction::Verify)) != 1
        || count(|step| matches!(step, ControllerInstruction::BufferVisible)) != 1
        || count(|step| matches!(step, ControllerInstruction::Dispatch { .. })) == 0
    {
        return Err(KernelError::at(
            FailureCode::InvalidPlan,
            Guard::G2FinitePlan,
            "plan requires dispatch, one verify, one buffer, and one final close_transaction",
        ));
    }
    let verify = instructions
        .iter()
        .position(|step| matches!(step, ControllerInstruction::Verify))
        .expect("verify count checked");
    let visible = instructions
        .iter()
        .position(|step| matches!(step, ControllerInstruction::BufferVisible))
        .expect("buffer count checked");
    if verify >= visible {
        return Err(KernelError::at(
            FailureCode::InvalidPlan,
            Guard::G2FinitePlan,
            "verification must precede any candidate-visible buffer",
        ));
    }
    let staged = instructions
        .iter()
        .enumerate()
        .filter_map(|(index, step)| {
            matches!(step, ControllerInstruction::StageEffect).then_some(index)
        })
        .collect::<Vec<_>>();
    if request.effect_class == EffectClass::ReadOnly && !staged.is_empty() {
        return Err(KernelError::at(
            FailureCode::InvalidPlan,
            Guard::G2FinitePlan,
            "read-only plan stages an effect",
        ));
    }
    if request.effect_class != EffectClass::ReadOnly
        && (staged.len() != 1 || staged[0] <= verify || staged[0] >= visible)
    {
        return Err(KernelError::at(
            FailureCode::InvalidPlan,
            Guard::G2FinitePlan,
            "mutating plan requires one staged effect after verification and before visibility",
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
    let cut = &request.evidence.semantic_cut;
    if is_zero(&cut.certificate_digest)
        || cut.plan_digest != request.binding.plan_digest
        || cut.fixed_model_digest != request.binding.fixed_model_digest
        || cut.comparison_identity_digest != request.binding.comparison_identity_digest
        || cut.semantic_authority != SemanticAuthority::OwnerScoped
    {
        return Err(KernelError::at(
            FailureCode::SemanticCutCrossing,
            Guard::G3Attribution,
            "semantic cut does not bind the plan, model, comparison identity, or owner scope",
        ));
    }
    if cut.attribution_class != AttributionClass::Fixed {
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
        || request.plan.instructions.len() > envelope.worker_steps as usize
    {
        return Err(KernelError::at(
            FailureCode::UnboundedWorker,
            Guard::G4Resources,
            "every resource/risk bound must be positive and cover all controller steps",
        ));
    }
    Ok(())
}

fn validate_g5(request: &PrepareRequest) -> Result<(), KernelError> {
    let SnapEvidence::Verified { certificate } = &request.evidence.snap else {
        return Ok(());
    };
    certificate.validate().map_err(|error| {
        KernelError::at(
            FailureCode::MissingSnapCertificate,
            Guard::G5RobustSnap,
            format!("Robust Snap certificate failed: {error}"),
        )
    })?;
    if *certificate.fiber.assembly_manifest_digest.as_bytes()
        != request.binding.assembly_manifest_digest
        || *certificate.fiber.source_image_digest.as_bytes() != request.binding.image_digest
        || *certificate.fiber.task_fingerprint.as_bytes() != request.binding.task_fingerprint_digest
    {
        return Err(KernelError::at(
            FailureCode::MissingSnapCertificate,
            Guard::G5RobustSnap,
            "Robust Snap certificate binds another assembly, image, or task",
        ));
    }
    if certificate.snap_level == SnapLevel::S0 {
        let selected = certificate.selected_effect.as_ref().ok_or_else(|| {
            KernelError::at(
                FailureCode::MissingSnapCertificate,
                Guard::G5RobustSnap,
                "S0 certificate has no selected effect",
            )
        })?;
        if request.evidence.safety_shield.action_digest != Some(*selected.effect_digest.as_bytes())
        {
            return Err(KernelError::at(
                FailureCode::MissingSnapCertificate,
                Guard::G5RobustSnap,
                "S0 selected effect differs from the shielded action",
            ));
        }
    }
    Ok(())
}

fn validate_g6(request: &PrepareRequest) -> Result<(), KernelError> {
    let shield = &request.evidence.safety_shield;
    if is_zero(&shield.shield_digest)
        || shield.state_snapshot != request.binding.state_snapshot_digest
        || shield.shield_digest
            != safety_shield_digest_v1(
                shield.kind,
                shield.state_snapshot,
                shield.action_digest,
                shield.evidence_digest,
                shield.verifier_digest,
                shield.acceptance_digest,
            )
    {
        return Err(KernelError::at(
            FailureCode::MissingSafetyShield,
            Guard::G6SafetyShield,
            "V2 shield identity or state binding is invalid",
        ));
    }
    match (request.effect_class, shield.kind) {
        (EffectClass::ReadOnly, SafetyShieldKindV1::ReadOnly) => {}
        (EffectClass::ReadOnly, SafetyShieldKindV1::AcceptedEffect)
        | (_, SafetyShieldKindV1::ReadOnly) => {
            return Err(KernelError::at(
                FailureCode::MissingSafetyShield,
                Guard::G6SafetyShield,
                "shield kind does not match the admitted effect class",
            ));
        }
        (_, SafetyShieldKindV1::AcceptedEffect) => {
            let accepted = shield.accepted_effect.as_ref().ok_or_else(|| {
                KernelError::at(
                    FailureCode::MissingSafetyShield,
                    Guard::G6SafetyShield,
                    "accepted-effect shield lost its zero-cert handle",
                )
            })?;
            accepted.validate().map_err(|error| {
                KernelError::at(
                    FailureCode::MissingSafetyShield,
                    Guard::G6SafetyShield,
                    format!("accepted effect failed validation: {error}"),
                )
            })?;
        }
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
            .is_none_or(|digest| is_zero(&digest))
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
    let performance = &request.evidence.performance;
    performance.validate().map_err(|error| {
        KernelError::at(
            FailureCode::PerformanceUnknown,
            Guard::G7Performance,
            format!("quality admission failed validation: {error}"),
        )
    })?;
    if *performance.comparison_identity_digest().as_bytes()
        != request.binding.comparison_identity_digest
        || (performance.evidence_class() != QualityEvidenceClassV1::Distributional
            && *performance.scope_digest().as_bytes() != request.binding.task_fingerprint_digest)
    {
        return Err(KernelError::at(
            FailureCode::PerformanceUnknown,
            Guard::G7Performance,
            "quality evidence binds another comparison identity or task",
        ));
    }
    if matches!(
        performance.evidence_class(),
        QualityEvidenceClassV1::ExactNeutral
            | QualityEvidenceClassV1::PointwiseDominance
            | QualityEvidenceClassV1::ScopedClassDominance
    ) && performance
        .candidate_identity_digest()
        .map(|digest| *digest.as_bytes())
        != Some(candidate_protocol_identity_v1(&request.binding))
    {
        return Err(KernelError::at(
            FailureCode::PerformanceUnknown,
            Guard::G7Performance,
            "quality evidence binds another candidate protocol identity",
        ));
    }
    let coherent = matches!(
        (
            performance.evidence_class(),
            performance.selection(),
            performance.guarantee(),
        ),
        (
            QualityEvidenceClassV1::ExactNeutral,
            QualitySelectionV1::Candidate,
            QualityGuaranteeV1::ExactSubstitution,
        ) | (
            QualityEvidenceClassV1::PointwiseDominance,
            QualitySelectionV1::Candidate,
            QualityGuaranteeV1::PointwiseNoWorse,
        ) | (
            QualityEvidenceClassV1::ScopedClassDominance,
            QualitySelectionV1::Candidate,
            QualityGuaranteeV1::ScopedClassNoWorse,
        ) | (
            QualityEvidenceClassV1::Distributional,
            QualitySelectionV1::FrozenBaseline,
            QualityGuaranteeV1::DistributionalOnly,
        ) | (
            QualityEvidenceClassV1::Unidentified,
            QualitySelectionV1::FrozenBaseline,
            QualityGuaranteeV1::Unidentified,
        )
    );
    if coherent {
        Ok(())
    } else {
        Err(KernelError::at(
            FailureCode::PerformanceUnknown,
            Guard::G7Performance,
            "quality evidence cannot authorize this strict candidate selection",
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
    bytes.extend_from_slice(b"zerostack.kernel.permit.v3\0");
    bytes.extend_from_slice(&request.admission_digest());
    bytes.extend_from_slice(&trace.digest());
    hash_bytes(&bytes)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StagedEffect {
    pub effect_digest: DigestV1,
    pub effect_class: EffectClass,
    pub acceptance_digest: Option<DigestV1>,
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
        if effect.effect_class != self.request.effect_class
            || is_zero(&effect.effect_digest)
            || self.request.evidence.safety_shield.action_digest != Some(effect.effect_digest)
            || self.request.evidence.safety_shield.acceptance_digest != effect.acceptance_digest
        {
            return Err(KernelError::execution(
                FailureCode::PlanStepMismatch,
                "staged effect does not match the admitted class, action, or acceptance",
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
        // Model-visible output is enforced separately by buffer_visible.
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

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestorationAccounting {
    pub attempted: u64,
    pub completed: u64,
    pub debt: u64,
}

/// G8 closure derived only from a validated zero-gate transaction receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionClosure {
    kind: ClosureKind,
    root: DigestV1,
    transaction_receipt_digest: DigestV1,
    action_digest: DigestV1,
    acceptance_digest: Option<DigestV1>,
    baseline_state: DigestV1,
    candidate_state: DigestV1,
    restoration_scope: RestorationScopeV1,
    external_restoration_debt_count: u16,
    restoration: RestorationAccounting,
}

impl TransactionClosure {
    pub fn from_receipt(receipt: TransactionReceiptV1) -> Result<Self, KernelError> {
        receipt.canonical_bytes().map_err(|error| {
            KernelError::at(
                FailureCode::IncompleteTransactionClosure,
                Guard::G8TransactionClosure,
                format!("transaction receipt failed validation: {error}"),
            )
        })?;
        let kind = match receipt.disposition() {
            TransactionDispositionV1::CandidateCommitted => ClosureKind::Commit,
            TransactionDispositionV1::BaselineRootRecovered => ClosureKind::Fallback,
        };
        let restoration = match kind {
            ClosureKind::Commit => RestorationAccounting::default(),
            ClosureKind::Fallback => RestorationAccounting {
                attempted: u64::from(receipt.resource_count()),
                completed: u64::from(
                    receipt
                        .resource_count()
                        .saturating_sub(receipt.external_restoration_debt_count()),
                ),
                debt: u64::from(receipt.external_restoration_debt_count()),
            },
        };
        Ok(Self {
            kind,
            root: *receipt.observed_root().as_bytes(),
            transaction_receipt_digest: *receipt.receipt_digest().as_bytes(),
            action_digest: *receipt.action_digest().as_bytes(),
            acceptance_digest: receipt.acceptance_digest().map(|digest| *digest.as_bytes()),
            baseline_state: *receipt.baseline_state().as_bytes(),
            candidate_state: *receipt.candidate_state().as_bytes(),
            restoration_scope: receipt.restoration_scope(),
            external_restoration_debt_count: receipt.external_restoration_debt_count(),
            restoration,
        })
    }

    pub const fn kind(&self) -> ClosureKind {
        self.kind
    }
    pub const fn root(&self) -> DigestV1 {
        self.root
    }
    pub const fn transaction_receipt_digest(&self) -> DigestV1 {
        self.transaction_receipt_digest
    }
    pub const fn restoration(&self) -> RestorationAccounting {
        self.restoration
    }
}

fn validate_closure(
    execution: &BrokeredExecution,
    closure: &TransactionClosure,
    failure: Option<FailureCode>,
) -> Result<(), KernelError> {
    if is_zero(&closure.root)
        || is_zero(&closure.transaction_receipt_digest)
        || is_zero(&closure.action_digest)
        || closure.baseline_state != execution.request.binding.state_snapshot_digest
        || closure.external_restoration_debt_count != 0
        || closure.restoration.debt != 0
    {
        return Err(KernelError::at(
            FailureCode::IncompleteTransactionClosure,
            Guard::G8TransactionClosure,
            "transaction receipt, state binding, or restoration debt is not closed",
        ));
    }
    match closure.kind {
        ClosureKind::Commit => {
            if failure.is_some()
                || closure.restoration != RestorationAccounting::default()
                || closure.root != closure.candidate_state
                || closure.restoration_scope != RestorationScopeV1::NotApplicableCandidateCommit
                || closure.acceptance_digest
                    != execution.request.evidence.safety_shield.acceptance_digest
                || execution.request.evidence.safety_shield.action_digest
                    != Some(closure.action_digest)
                || execution.request.evidence.performance.selection()
                    != QualitySelectionV1::Candidate
            {
                return Err(KernelError::at(
                    FailureCode::IncompleteTransactionClosure,
                    Guard::G8TransactionClosure,
                    "candidate commit is not bound to its accepted action, quality, and root",
                ));
            }
        }
        ClosureKind::Fallback => {
            if closure.root != closure.baseline_state
                || closure.restoration.attempted == 0
                || closure.restoration.completed != closure.restoration.attempted
                || closure.restoration_scope != RestorationScopeV1::DeclaredEffectClosure
                || (failure.is_none()
                    && execution.request.evidence.performance.selection()
                        != QualitySelectionV1::FrozenBaseline)
            {
                return Err(KernelError::at(
                    FailureCode::UnaccountedFallback,
                    Guard::G8TransactionClosure,
                    "fallback did not recover the declared baseline closure",
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
    pub state_snapshot_digest: DigestV1,
    pub task_fingerprint_digest: DigestV1,
    pub plan_digest: DigestV1,
    pub fixed_model_digest: DigestV1,
    pub comparison_identity_digest: DigestV1,
    pub artifact_set_digest: DigestV1,
    pub semantic_cut_certificate_digest: DigestV1,
    pub snap_certificate_digest: Option<DigestV1>,
    pub safety_shield_digest: DigestV1,
    pub quality_admission: QualityAdmissionRecordV1,
    pub final_quality_selection: QualitySelectionV1,
    pub transaction_receipt_digest: DigestV1,
    pub attribution_class: AttributionClass,
    pub effect_class: EffectClass,
    pub resource_envelope: WorkerEnvelope,
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
        state_snapshot_digest: record.state_snapshot_digest,
        task_fingerprint_digest: record.task_fingerprint_digest,
        plan_digest: record.plan_digest,
        fixed_model_digest: record.fixed_model_digest,
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
        record.state_snapshot_digest,
        record.task_fingerprint_digest,
        record.plan_digest,
        record.fixed_model_digest,
        record.comparison_identity_digest,
        record.artifact_set_digest,
        record.semantic_cut_certificate_digest,
        record.safety_shield_digest,
        *record.quality_admission.admission_digest.as_bytes(),
        record.transaction_receipt_digest,
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
        || record
            .snap_certificate_digest
            .is_some_and(|digest| is_zero(&digest))
        || binding.digest() != record.binding_digest
        || record.attribution_class != AttributionClass::Fixed
        || envelope_has_zero(record.resource_envelope)
        || !quality_receipt_fields_valid(record)
    {
        return Err(KernelError::execution(
            FailureCode::ForgedReceipt,
            "receipt contains a zero, noncanonical, changed-attribution, or mismatched binding",
        ));
    }
    let accounted = record
        .restoration
        .completed
        .checked_add(record.restoration.debt);
    let closure_valid = match record.kind {
        ReceiptKind::Commit => {
            record.failure_code.is_none() && record.restoration == RestorationAccounting::default()
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
        record.artifact_set_digest,
        record.semantic_cut_certificate_digest,
        record.snap_certificate_digest,
        record.safety_shield_digest,
        &record.quality_admission,
        record.final_quality_selection,
        record.transaction_receipt_digest,
        record.attribution_class,
        record.effect_class,
        record.resource_envelope,
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

fn quality_receipt_fields_valid(record: &ReceiptRecord) -> bool {
    record.quality_admission.validate().is_ok()
        && *record
            .quality_admission
            .comparison_identity_digest
            .as_bytes()
            == record.comparison_identity_digest
        && (record.quality_admission.evidence_class == QualityEvidenceClassV1::Distributional
            || *record.quality_admission.scope_digest.as_bytes() == record.task_fingerprint_digest)
        && (!matches!(
            record.quality_admission.evidence_class,
            QualityEvidenceClassV1::ExactNeutral
                | QualityEvidenceClassV1::PointwiseDominance
                | QualityEvidenceClassV1::ScopedClassDominance
        ) || record
            .quality_admission
            .candidate_identity_digest
            .map(|digest| *digest.as_bytes())
            == Some(candidate_protocol_identity_v1(&ExecutionBinding {
                schema_version: record.schema_version,
                assembly_manifest_digest: record.assembly_manifest_digest,
                source_tree_digest: record.source_tree_digest,
                source_repository_heads: record.source_repository_heads.clone(),
                image_digest: record.image_digest,
                state_snapshot_digest: record.state_snapshot_digest,
                task_fingerprint_digest: record.task_fingerprint_digest,
                plan_digest: record.plan_digest,
                fixed_model_digest: record.fixed_model_digest,
                comparison_identity_digest: record.comparison_identity_digest,
                predecessor_receipt_head: record.predecessor_receipt_head,
            })))
        && matches!(
            (record.kind, record.final_quality_selection),
            (ReceiptKind::Commit, QualitySelectionV1::Candidate)
                | (ReceiptKind::Fallback, QualitySelectionV1::FrozenBaseline)
        )
        && (record.kind != ReceiptKind::Commit
            || record.quality_admission.selection == QualitySelectionV1::Candidate)
}

fn envelope_has_zero(envelope: WorkerEnvelope) -> bool {
    envelope.fuel == 0
        || envelope.deadline_ms == 0
        || envelope.io_bytes == 0
        || envelope.output_bytes == 0
        || envelope.memory_bytes == 0
        || envelope.processes == 0
        || envelope.risk_units == 0
        || envelope.worker_steps == 0
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
        let artifact_set_digest = self.request.evidence.artifacts.artifact_set_digest;
        let semantic_cut_certificate_digest = self.request.evidence.semantic_cut.certificate_digest;
        let snap_certificate_digest = match &self.request.evidence.snap {
            SnapEvidence::NotClaimed => None,
            SnapEvidence::Verified { certificate } => {
                Some(*certificate.certificate_digest.as_bytes())
            }
        };
        let safety_shield_digest = self.request.evidence.safety_shield.shield_digest;
        let quality_admission = self.request.evidence.performance.record();
        let final_quality_selection = match kind {
            ReceiptKind::Commit => QualitySelectionV1::Candidate,
            ReceiptKind::Fallback => QualitySelectionV1::FrozenBaseline,
        };
        let transaction_receipt_digest = self.closure.transaction_receipt_digest;
        let attribution_class = self.request.evidence.semantic_cut.attribution_class;
        let effect_class = self.request.effect_class;
        let resource_envelope = self.request.envelope;
        let receipt_head = receipt_digest(
            kind,
            self.permit_id,
            &self.request.binding,
            admission_digest,
            artifact_set_digest,
            semantic_cut_certificate_digest,
            snap_certificate_digest,
            safety_shield_digest,
            &quality_admission,
            final_quality_selection,
            transaction_receipt_digest,
            attribution_class,
            effect_class,
            resource_envelope,
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
            artifact_set_digest,
            semantic_cut_certificate_digest,
            snap_certificate_digest,
            safety_shield_digest,
            quality_admission,
            final_quality_selection,
            transaction_receipt_digest,
            attribution_class,
            effect_class,
            resource_envelope,
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
    artifact_set_digest: DigestV1,
    semantic_cut_certificate_digest: DigestV1,
    snap_certificate_digest: Option<DigestV1>,
    safety_shield_digest: DigestV1,
    quality_admission: QualityAdmissionRecordV1,
    final_quality_selection: QualitySelectionV1,
    transaction_receipt_digest: DigestV1,
    attribution_class: AttributionClass,
    effect_class: EffectClass,
    resource_envelope: WorkerEnvelope,
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
            state_snapshot_digest: self.binding.state_snapshot_digest,
            task_fingerprint_digest: self.binding.task_fingerprint_digest,
            plan_digest: self.binding.plan_digest,
            fixed_model_digest: self.binding.fixed_model_digest,
            comparison_identity_digest: self.binding.comparison_identity_digest,
            artifact_set_digest: self.artifact_set_digest,
            semantic_cut_certificate_digest: self.semantic_cut_certificate_digest,
            snap_certificate_digest: self.snap_certificate_digest,
            safety_shield_digest: self.safety_shield_digest,
            quality_admission: self.quality_admission.clone(),
            final_quality_selection: self.final_quality_selection,
            transaction_receipt_digest: self.transaction_receipt_digest,
            attribution_class: self.attribution_class,
            effect_class: self.effect_class,
            resource_envelope: self.resource_envelope,
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
    /// Releases buffered output without making a filesystem durability claim.
    pub fn publish(self) -> PublishedCommit {
        PublishedCommit {
            visible_bytes: self.buffered_visible,
            approved_effects: self.staged_effects,
            receipt_head: self.common.receipt_head,
            successor_root: self.common.successor_root,
            durability: PublicationDurabilityV1::JournalRootCommitted {
                transaction_receipt_digest: self.common.transaction_receipt_digest,
            },
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationDurabilityV1 {
    BufferedOnly,
    /// A zero-store journal root commit. This is not a native filesystem durability claim.
    JournalRootCommitted {
        transaction_receipt_digest: DigestV1,
    },
    JournalVerified {
        evidence_digest: DigestV1,
        durable_profile_digest: DigestV1,
    },
}

#[derive(Debug)]
pub struct PublishedCommit {
    pub visible_bytes: Vec<u8>,
    pub approved_effects: Vec<StagedEffect>,
    pub receipt_head: DigestV1,
    pub successor_root: DigestV1,
    pub durability: PublicationDurabilityV1,
}

#[allow(clippy::too_many_arguments)]
fn receipt_digest(
    kind: ReceiptKind,
    permit_id: DigestV1,
    binding: &ExecutionBinding,
    admission_digest: DigestV1,
    artifact_set_digest: DigestV1,
    semantic_cut_certificate_digest: DigestV1,
    snap_certificate_digest: Option<DigestV1>,
    safety_shield_digest: DigestV1,
    quality_admission: &QualityAdmissionRecordV1,
    final_quality_selection: QualitySelectionV1,
    transaction_receipt_digest: DigestV1,
    attribution_class: AttributionClass,
    effect_class: EffectClass,
    envelope: WorkerEnvelope,
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
    bytes.extend_from_slice(b"zerostack.kernel.receipt.v3\0");
    bytes.extend_from_slice(&TWO_PHASE_SCHEMA_VERSION.to_be_bytes());
    bytes.push(kind as u8);
    bytes.extend_from_slice(&permit_id);
    bytes.extend_from_slice(&binding.digest());
    bytes.extend_from_slice(&admission_digest);
    bytes.extend_from_slice(&artifact_set_digest);
    bytes.extend_from_slice(&semantic_cut_certificate_digest);
    append_optional_digest(&mut bytes, snap_certificate_digest);
    bytes.extend_from_slice(&safety_shield_digest);
    bytes.extend_from_slice(quality_admission.admission_digest.as_bytes());
    bytes.push(match final_quality_selection {
        QualitySelectionV1::Candidate => 0,
        QualitySelectionV1::FrozenBaseline => 1,
    });
    bytes.extend_from_slice(&transaction_receipt_digest);
    bytes.push(match attribution_class {
        AttributionClass::Fixed => 0,
        AttributionClass::Changed => 1,
    });
    bytes.push(effect_class_tag(effect_class));
    bytes.extend_from_slice(&envelope.fuel.to_be_bytes());
    bytes.extend_from_slice(&envelope.deadline_ms.to_be_bytes());
    bytes.extend_from_slice(&envelope.io_bytes.to_be_bytes());
    bytes.extend_from_slice(&envelope.output_bytes.to_be_bytes());
    bytes.extend_from_slice(&envelope.memory_bytes.to_be_bytes());
    bytes.extend_from_slice(&envelope.processes.to_be_bytes());
    bytes.extend_from_slice(&envelope.risk_units.to_be_bytes());
    bytes.extend_from_slice(&envelope.worker_steps.to_be_bytes());
    bytes.push(surface as u8);
    append_optional_digest(&mut bytes, verification_digest);
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
    bytes.extend_from_slice(b"zerostack.kernel.effects.v3\0");
    bytes.extend_from_slice(&(effects.len() as u64).to_be_bytes());
    for effect in effects {
        bytes.extend_from_slice(&effect.effect_digest);
        bytes.push(match effect.effect_class {
            EffectClass::ReadOnly => 0,
            EffectClass::ReversibleMutation => 1,
            EffectClass::ApprovalRequiredMutation => 2,
            EffectClass::Irreversible => 3,
        });
        append_optional_digest(&mut bytes, effect.acceptance_digest);
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
    use crate::quality::{
        DistributionalCertificateV1, DistributionalClaimV1, ExactNeutralCertificateV1,
        FrozenBaselineV1, MetricOrderV1, PointwiseDominanceCertificateV1, ProtectedMetricV1,
        QualityEvidenceV1, QualityPairV1,
    };
    use crate::transaction::RestorationScopeV1;
    use std::borrow::Cow;
    use zero_abi::{
        sha256, CwirVerifierClassV1, EffectProgramV1, EffectRollbackV1, EffectTargetV1,
        EffectVerificationPlanV1, EffectVerificationStepV1, ProtectedEffectClassV1,
        ProtectedEffectSet, ProtectedEffectV1, TypedEffectOperationV1, WorldFiberDescriptor,
        ROBUST_SNAP_MODEL_VERSION,
    };
    use zero_cert::{
        accept_effect_verification_v1, verify, CompletenessWitness, EffectVerificationOutcomeV1,
        EvidenceCertificate, ObjectId, OperatorLock, Provenance, Query, Resolver, SpanRef,
    };

    fn digest(byte: u8) -> DigestV1 {
        [byte; 32]
    }

    fn abi(byte: u8) -> AbiDigestV1 {
        AbiDigestV1::from_bytes(digest(byte))
    }

    struct Resident<'a> {
        bytes: &'a [u8],
    }

    impl Resolver for Resident<'_> {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (sha256(self.bytes) == object_id.0).then_some(self.bytes)
        }
        fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "read-span").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "tree-sitter").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "zero-index").then_some("2")
        }
    }

    fn certificate(bytes: &[u8]) -> EvidenceCertificate<'_> {
        let object = sha256(bytes);
        let span = SpanRef {
            object_id: ObjectId(object),
            object_digest: object,
            byte_start: 0,
            byte_len: bytes.len() as u64,
            span_digest: object,
        };
        EvidenceCertificate {
            query: Query::ReadSpan(span.clone()),
            spans: vec![span],
            payload: Cow::Borrowed(bytes),
            provenance: Provenance {
                parser_id: "tree-sitter".into(),
                parser_version: "1".into(),
                index_id: "zero-index".into(),
                index_version: "2".into(),
                operator_id: "read-span".into(),
                operator_version: "1".into(),
            },
            completeness: CompletenessWitness::ReadSpan {
                operator: OperatorLock {
                    operator_id: "read-span".into(),
                    operator_version: "1".into(),
                },
            },
            input_token_cost: 1,
            backend_work_units: 1,
        }
    }

    fn effect_program(snapshot: DigestV1) -> EffectProgramV1 {
        let snapshot = AbiDigestV1::from_bytes(snapshot);
        let target = EffectTargetV1 {
            owner: ArtifactOwnerV1::FsZero,
            target_digest: abi(10),
            required_snapshot: snapshot,
        };
        EffectProgramV1::new(
            snapshot,
            "kernel_test",
            vec![target],
            vec![],
            vec![TypedEffectOperationV1::ReplaceExactFile {
                target: abi(10),
                expected_before: abi(11),
                replacement: abi(12),
            }],
            vec![],
            EffectVerificationPlanV1::new(vec![EffectVerificationStepV1 {
                verifier_digest: abi(20),
                predicate_digest: abi(21),
                environment_digest: abi(22),
                required_snapshot: snapshot,
                verifier_class: CwirVerifierClassV1::ExactChecker,
            }])
            .unwrap(),
            EffectRollbackV1::Journaled,
        )
        .unwrap()
    }

    fn accepted_effect() -> EffectAcceptedV1 {
        let bytes = b"exact kernel evidence";
        let certificate = certificate(bytes);
        let resident = Resident { bytes };
        let verified = verify(&certificate, &resident).unwrap();
        let program = effect_program(digest(13));
        let outcome = accept_effect_verification_v1(
            abi(70),
            &program,
            abi(71),
            abi(21),
            abi(13),
            abi(20),
            &verified,
        )
        .unwrap();
        let EffectVerificationOutcomeV1::Accepted(accepted) = outcome else {
            panic!("expected accepted effect")
        };
        accepted
    }

    fn read_only_shield() -> SafetyShieldEvidenceV1 {
        let bytes = b"verified read-only kernel evidence";
        let certificate = certificate(bytes);
        let resident = Resident { bytes };
        let verified = verify(&certificate, &resident).unwrap();
        SafetyShieldEvidenceV1::from_read_only_verified(digest(13), digest(72), &verified).unwrap()
    }

    fn semantic_cut(plan_digest: DigestV1) -> SemanticCutEvidenceV1 {
        let bytes = b"verified semantic-cut evidence";
        let certificate = certificate(bytes);
        let resident = Resident { bytes };
        let verified = verify(&certificate, &resident).unwrap();
        SemanticCutEvidenceV1::verify_owner_scoped(plan_digest, digest(15), digest(4), &verified)
            .unwrap()
    }

    fn artifacts() -> CanonicalArtifactSetV1 {
        let assembly = abi(1);
        let source = abi(2);
        let profile = DurableProfileV1::portable_strict();
        let specifications = [
            (ArtifactOwnerV1::FsZero, ZbfArtifactKindV1::FsPack, 31),
            (ArtifactOwnerV1::GraphZero, ZbfArtifactKindV1::GraphPack, 32),
            (ArtifactOwnerV1::TokenZero, ZbfArtifactKindV1::TokenPack, 33),
        ];
        let inputs = specifications
            .into_iter()
            .map(|(owner, kind, producer)| PeerArtifactInputV1 {
                bytes: ZbfObjectV1::new_leaf(
                    kind,
                    owner,
                    assembly,
                    profile,
                    source,
                    abi(producer),
                    vec![producer],
                )
                .unwrap()
                .to_bytes(profile)
                .unwrap(),
                expected_owner: owner,
                expected_kind: kind,
                expected_producer_contract_digest: digest(producer),
            })
            .collect();
        CanonicalArtifactSetV1::verify(digest(1), digest(2), inputs).unwrap()
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

    fn quality_admission(candidate_identity: AbiDigestV1) -> PerformanceAdmission {
        let certificate = ExactNeutralCertificateV1::verify(
            abi(14),
            abi(4),
            abi(16),
            candidate_identity,
            abi(17),
            abi(17),
            abi(18),
            abi(18),
            abi(19),
            abi(19),
        )
        .unwrap();
        QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::ExactNeutral(certificate),
            FrozenBaselineV1::new(abi(16), abi(19), abi(20)).unwrap(),
        )
        .unwrap()
    }

    fn pointwise_quality_admission(candidate_identity: AbiDigestV1) -> PerformanceAdmission {
        let pair = QualityPairV1::new(
            abi(14),
            abi(4),
            abi(16),
            candidate_identity,
            abi(19),
            abi(21),
            abi(22),
            abi(26),
            vec![ProtectedMetricV1 {
                metric_id: "protected_outcome".into(),
                order: MetricOrderV1::AtLeast,
                baseline_value: 1,
                candidate_value: 2,
            }],
        )
        .unwrap();
        let bytes = pair.canonical_bytes().unwrap();
        let evidence_certificate = certificate(&bytes);
        let resident = Resident { bytes: &bytes };
        let verified = verify(&evidence_certificate, &resident).unwrap();
        let dominance = PointwiseDominanceCertificateV1::verify(&pair, abi(23), &verified).unwrap();
        QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::PointwiseDominance(dominance),
            FrozenBaselineV1::new(abi(16), abi(19), abi(20)).unwrap(),
        )
        .unwrap()
    }

    fn distributional_quality_admission() -> PerformanceAdmission {
        let claim = DistributionalClaimV1::new(
            abi(24),
            abi(4),
            abi(25),
            abi(16),
            abi(19),
            abi(22),
            abi(26),
            abi(27),
            100,
            10,
            2,
            88,
            80_000,
            50_000,
            950_000,
        )
        .unwrap();
        let bytes = claim.canonical_bytes().unwrap();
        let evidence_certificate = certificate(&bytes);
        let resident = Resident { bytes: &bytes };
        let verified = verify(&evidence_certificate, &resident).unwrap();
        let distributional = DistributionalCertificateV1::verify(&claim, &verified).unwrap();
        QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::Distributional(distributional),
            FrozenBaselineV1::new(abi(16), abi(19), abi(20)).unwrap(),
        )
        .unwrap()
    }

    fn request(surface: ExecutionSurface, effect_class: EffectClass) -> PrepareRequest {
        let plan = plan(effect_class);
        let plan_digest = plan.digest();
        let artifacts = artifacts();
        let image_digest = artifacts.image_digest;
        let safety_shield = if effect_class == EffectClass::ReadOnly {
            read_only_shield()
        } else {
            SafetyShieldEvidenceV1::from_effect_accepted(accepted_effect()).unwrap()
        };
        let binding = ExecutionBinding {
            schema_version: TWO_PHASE_SCHEMA_VERSION,
            assembly_manifest_digest: digest(1),
            source_tree_digest: digest(2),
            source_repository_heads: vec![SourceHead {
                repository: "ZeroStack".into(),
                head: "87c8ef5df0699b6345e4a829876b3f086f9c3ae5".into(),
            }],
            image_digest,
            state_snapshot_digest: digest(13),
            task_fingerprint_digest: digest(14),
            plan_digest,
            fixed_model_digest: digest(15),
            comparison_identity_digest: digest(4),
            predecessor_receipt_head: digest(5),
        };
        let candidate_identity = AbiDigestV1::from_bytes(candidate_protocol_identity_v1(&binding));
        PrepareRequest {
            binding,
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
                artifacts,
                semantic_cut: semantic_cut(plan_digest),
                snap: SnapEvidence::NotClaimed,
                safety_shield,
                approval_grant_digest: (effect_class == EffectClass::ApprovalRequiredMutation)
                    .then(|| digest(12)),
                irreversible_pre_action_evidence_digest: if effect_class
                    == EffectClass::Irreversible
                {
                    Some(digest(8))
                } else {
                    None
                },
                performance: quality_admission(candidate_identity),
            },
        }
    }

    fn snap_certificate(effect_digest: DigestV1, image_digest: DigestV1) -> RobustSnapCertificate {
        let selected = ProtectedEffectV1 {
            effect_digest: AbiDigestV1::from_bytes(effect_digest),
            effect_class: ProtectedEffectClassV1::ReversibleMutation,
        };
        let worlds = vec![abi(40), abi(41)];
        RobustSnapCertificate::create_s0(
            WorldFiberDescriptor {
                model_version: ROBUST_SNAP_MODEL_VERSION.into(),
                assembly_manifest_digest: abi(1),
                source_image_digest: AbiDigestV1::from_bytes(image_digest),
                task_fingerprint: abi(14),
                assumptions: vec!["fixed inputs".into()],
                worlds: worlds.clone(),
            },
            worlds
                .into_iter()
                .map(|world_id| ProtectedEffectSet {
                    world_id,
                    effects: vec![selected.clone()],
                })
                .collect(),
            vec![selected.clone()],
            vec![selected.clone()],
            selected,
        )
        .unwrap()
    }

    fn action_and_acceptance() -> (DigestV1, DigestV1) {
        let accepted = accepted_effect();
        (
            *accepted.action_digest().as_bytes(),
            *accepted.acceptance_digest().as_bytes(),
        )
    }

    fn commit_closure() -> TransactionClosure {
        let (action_digest, acceptance_digest) = action_and_acceptance();
        TransactionClosure {
            kind: ClosureKind::Commit,
            root: digest(11),
            transaction_receipt_digest: digest(17),
            action_digest,
            acceptance_digest: Some(acceptance_digest),
            baseline_state: digest(13),
            candidate_state: digest(11),
            restoration_scope: RestorationScopeV1::NotApplicableCandidateCommit,
            external_restoration_debt_count: 0,
            restoration: RestorationAccounting::default(),
        }
    }

    fn fallback_closure() -> TransactionClosure {
        TransactionClosure {
            kind: ClosureKind::Fallback,
            root: digest(13),
            transaction_receipt_digest: digest(18),
            action_digest: digest(19),
            acceptance_digest: None,
            baseline_state: digest(13),
            candidate_state: digest(11),
            restoration_scope: RestorationScopeV1::DeclaredEffectClosure,
            external_restoration_debt_count: 0,
            restoration: RestorationAccounting {
                attempted: 1,
                completed: 1,
                debt: 0,
            },
        }
    }

    fn staged(effect_class: EffectClass) -> StagedEffect {
        let (effect_digest, acceptance_digest) = action_and_acceptance();
        StagedEffect {
            effect_digest,
            effect_class,
            acceptance_digest: Some(acceptance_digest),
            approval_grant_digest: (effect_class == EffectClass::ApprovalRequiredMutation)
                .then(|| digest(12)),
            pre_action_evidence_digest: (effect_class == EffectClass::Irreversible)
                .then(|| digest(8)),
        }
    }

    fn execute_request(
        request: PrepareRequest,
        closure: TransactionClosure,
    ) -> Result<ReadyToFinalize, KernelError> {
        let permit = prepare(request).map_err(|failure| failure.into_parts().0)?;
        validate_permit_record(&permit.record())?;
        let mut execution = permit.start();
        execution.dispatch(
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
        )?;
        execution.deterministic_transform()?;
        execution.record_verification(digest(9))?;
        execution.stage_effect(staged(EffectClass::ReversibleMutation))?;
        assert_eq!(
            execution.reject_early_publish().code,
            FailureCode::EarlyVisibleByte
        );
        execution.buffer_visible(b"accepted")?;
        execution.close_transaction(closure)
    }

    fn run_to_ready(surface: ExecutionSurface) -> ReadyToFinalize {
        execute_request(
            request(surface, EffectClass::ReversibleMutation),
            commit_closure(),
        )
        .unwrap()
    }

    #[test]
    fn state_machine_contract_digest_is_stable() {
        assert_eq!(
            two_phase_contract_digest_v2(),
            [
                0xe8, 0x4b, 0x6e, 0xe8, 0x08, 0x63, 0x79, 0xc6, 0x37, 0xcf, 0x50, 0x65, 0x1e, 0x16,
                0x6a, 0x2e, 0xca, 0x62, 0x58, 0xfc, 0x49, 0xaa, 0xdb, 0x40, 0x52, 0x69, 0x68, 0xf7,
                0xa5, 0x30, 0xa6, 0x87,
            ]
        );
        assert_eq!(
            two_phase_contract_digest_v3(),
            [
                0x12, 0x18, 0x25, 0xd4, 0x3e, 0xee, 0x2a, 0xbc, 0xe2, 0x6a, 0x88, 0x6b, 0x67, 0xd5,
                0xde, 0xf6, 0x44, 0x03, 0x74, 0xc3, 0x98, 0xe8, 0x4b, 0x77, 0x4d, 0x77, 0x28, 0xd0,
                0x32, 0x13, 0x99, 0x34
            ]
        );
    }

    #[test]
    fn state_machine_quality_envelope_guards_candidate_and_distributional_fallback() {
        let mut pointwise = request(ExecutionSurface::Mcp, EffectClass::ReversibleMutation);
        let candidate_identity =
            AbiDigestV1::from_bytes(candidate_protocol_identity_v1(&pointwise.binding));
        pointwise.evidence.performance = pointwise_quality_admission(candidate_identity);
        let FinalReceipt::Commit(receipt) = execute_request(pointwise, commit_closure())
            .unwrap()
            .finalize()
            .unwrap()
        else {
            panic!("pointwise candidate must commit")
        };
        let record = receipt.record();
        assert_eq!(
            record.quality_admission.evidence_class,
            QualityEvidenceClassV1::PointwiseDominance
        );
        assert_eq!(
            record.quality_admission.selection,
            QualitySelectionV1::Candidate
        );
        assert_eq!(
            record.final_quality_selection,
            QualitySelectionV1::Candidate
        );
        assert_eq!(
            record.quality_admission.guarantee,
            QualityGuaranteeV1::PointwiseNoWorse
        );
        assert!(record.quality_admission.strict_improvement);
        validate_receipt_record(&record).unwrap();

        let mut distributional = request(ExecutionSurface::Mcp, EffectClass::ReversibleMutation);
        distributional.evidence.performance = distributional_quality_admission();
        let FinalReceipt::Fallback(receipt) = execute_request(distributional, fallback_closure())
            .unwrap()
            .finalize()
            .unwrap()
        else {
            panic!("distributional evidence must select the frozen baseline")
        };
        let record = receipt.record();
        assert_eq!(
            record.quality_admission.evidence_class,
            QualityEvidenceClassV1::Distributional
        );
        assert_eq!(
            record.quality_admission.selection,
            QualitySelectionV1::FrozenBaseline
        );
        assert_eq!(
            record.final_quality_selection,
            QualitySelectionV1::FrozenBaseline
        );
        assert_eq!(
            record.quality_admission.guarantee,
            QualityGuaranteeV1::DistributionalOnly
        );
        assert!(!record.quality_admission.strict_improvement);
        validate_receipt_record(&record).unwrap();

        let mut candidate_mismatch =
            request(ExecutionSurface::Mcp, EffectClass::ReversibleMutation);
        candidate_mismatch.evidence.performance = quality_admission(abi(99));
        assert_eq!(
            prepare(candidate_mismatch).unwrap_err().error().code,
            FailureCode::PerformanceUnknown
        );

        let mismatched = ExactNeutralCertificateV1::verify(
            abi(14),
            abi(99),
            abi(16),
            abi(28),
            abi(17),
            abi(17),
            abi(18),
            abi(18),
            abi(19),
            abi(19),
        )
        .unwrap();
        let mut request = request(ExecutionSurface::Mcp, EffectClass::ReversibleMutation);
        request.evidence.performance = QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::ExactNeutral(mismatched),
            FrozenBaselineV1::new(abi(16), abi(19), abi(20)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            prepare(request).unwrap_err().error().code,
            FailureCode::PerformanceUnknown
        );
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
        assert_eq!(record.state_snapshot_digest, digest(13));
        assert_eq!(record.predecessor_receipt_head, digest(5));
        assert_eq!(record.successor_root, digest(11));
        assert_eq!(record.transaction_receipt_digest, digest(17));
        validate_receipt_record(&record).unwrap();
        let published = receipt.publish();
        assert_eq!(
            published.durability,
            PublicationDurabilityV1::JournalRootCommitted {
                transaction_receipt_digest: digest(17)
            }
        );
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
    fn state_machine_strict_artifacts_omitted_guards_and_forged_predecessors_fail() {
        let malformed = vec![PeerArtifactInputV1 {
            bytes: b"not-zbf".to_vec(),
            expected_owner: ArtifactOwnerV1::FsZero,
            expected_kind: ZbfArtifactKindV1::FsPack,
            expected_producer_contract_digest: digest(31),
        }];
        assert_eq!(
            CanonicalArtifactSetV1::verify(digest(1), digest(2), malformed)
                .unwrap_err()
                .code,
            FailureCode::InvalidSourceIdentity
        );

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
    fn state_machine_forged_permit_unbounded_worker_semantic_cut_and_image_fail() {
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
        cut.evidence.semantic_cut.semantic_authority = SemanticAuthority::HiddenTaskSelector;
        assert_eq!(
            prepare(cut).unwrap_err().error().code,
            FailureCode::SemanticCutCrossing
        );
        let mut image = request(ExecutionSurface::Pi, EffectClass::ReadOnly);
        image.binding.image_digest = digest(99);
        assert_eq!(
            prepare(image).unwrap_err().error().code,
            FailureCode::CoherenceFailure
        );
        let mut snap = request(ExecutionSurface::Pi, EffectClass::ReversibleMutation);
        snap.evidence.snap = SnapEvidence::Verified {
            certificate: snap_certificate(digest(99), snap.binding.image_digest),
        };
        assert_eq!(
            prepare(snap).unwrap_err().error().code,
            FailureCode::MissingSnapCertificate
        );
        let mut snap = request(ExecutionSurface::Pi, EffectClass::ReversibleMutation);
        let action = snap.evidence.safety_shield.action_digest.unwrap();
        snap.evidence.snap = SnapEvidence::Verified {
            certificate: snap_certificate(action, snap.binding.image_digest),
        };
        prepare(snap).unwrap();
        let mut order = request(ExecutionSurface::Pi, EffectClass::ReversibleMutation);
        order.plan.instructions.swap(2, 3);
        order.binding.plan_digest = order.plan.digest();
        order.evidence.semantic_cut.plan_digest = order.binding.plan_digest;
        assert_eq!(
            prepare(order).unwrap_err().error().code,
            FailureCode::InvalidPlan
        );
    }

    #[test]
    fn state_machine_buffer_overflow_falls_back_only_to_bound_baseline() {
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
        let mut bad = fallback_closure();
        bad.root = digest(12);
        assert_eq!(
            execution.abort(error.code, bad).unwrap_err().code,
            FailureCode::UnaccountedFallback
        );

        let permit = prepare(request(ExecutionSurface::ClaudeCode, EffectClass::ReadOnly)).unwrap();
        let execution = permit.start();
        let ready = execution
            .abort(FailureCode::BufferOverflow, fallback_closure())
            .unwrap();
        let FinalReceipt::Fallback(receipt) = ready.finalize().unwrap() else {
            panic!("expected fallback")
        };
        receipt.trace().verify_complete().unwrap();
        let record = receipt.record();
        assert_eq!(record.failure_code, Some(FailureCode::BufferOverflow));
        assert_eq!(record.successor_root, digest(13));
        assert_eq!(
            record.quality_admission.selection,
            QualitySelectionV1::Candidate
        );
        assert_eq!(
            record.final_quality_selection,
            QualitySelectionV1::FrozenBaseline
        );
        validate_receipt_record(&record).unwrap();
    }

    #[test]
    fn state_machine_effects_require_matching_acceptance_and_pre_action_evidence() {
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
        let mut effect = staged(EffectClass::Irreversible);
        effect.pre_action_evidence_digest = Some(digest(99));
        assert_eq!(
            execution.stage_effect(effect).unwrap_err().code,
            FailureCode::IrreversiblePreEvidenceEffect
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
        let mut effect = staged(EffectClass::ApprovalRequiredMutation);
        effect.approval_grant_digest = Some(digest(99));
        assert_eq!(
            execution.stage_effect(effect).unwrap_err().code,
            FailureCode::MissingApprovalGrant
        );
    }

    #[test]
    fn state_machine_admission_and_receipt_commitments_reject_tampering() {
        let mut missing = request(ExecutionSurface::Mcp, EffectClass::ReadOnly);
        missing.binding.predecessor_receipt_head = [0; 32];
        assert_eq!(
            prepare(missing).unwrap_err().error().code,
            FailureCode::MissingBinding
        );

        let base = request(ExecutionSurface::Mcp, EffectClass::ReversibleMutation);
        let base_digest = base.admission_digest();
        let mut changed_envelope = base.clone();
        changed_envelope.envelope.fuel += 1;
        assert_ne!(base_digest, changed_envelope.admission_digest());
        let mut changed_evidence = base.clone();
        changed_evidence.evidence.safety_shield.shield_digest = digest(99);
        assert_ne!(base_digest, changed_evidence.admission_digest());
        assert_eq!(
            prepare(changed_evidence).unwrap_err().error().code,
            FailureCode::MissingSafetyShield
        );

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
        receipt_record.transaction_receipt_digest[0] ^= 1;
        assert_eq!(
            validate_receipt_record(&receipt_record).unwrap_err().code,
            FailureCode::ForgedReceipt
        );
        let mut quality_tamper = receipt.record();
        quality_tamper.quality_admission.selection = QualitySelectionV1::FrozenBaseline;
        assert_eq!(
            validate_receipt_record(&quality_tamper).unwrap_err().code,
            FailureCode::ForgedReceipt
        );
    }
}
