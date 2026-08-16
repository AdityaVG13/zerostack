//! Assembly-bound two-phase execution kernel.
//!
//! `ExecutionPermit`, `BrokeredExecution`, `ReadyToFinalize`, and the final
//! receipts are linear capabilities. Their fields are private, they are not
//! cloneable, and only the preceding phase can construct the next phase.

use crate::deoptimization::{deoptimization_contract_digest_v1, BaselineExecutionReceiptV1};
use crate::quality::{
    quality_envelope_contract_digest_v1, QualityAdmissionRecordV1, QualityAdmissionV1,
    QualityEvidenceClassV1, QualityGuaranteeV1, QualitySelectionV1,
};
use crate::semantic_cut::{
    semantic_cut_contract_digest_v1, SemanticCutCertificateRecordV1, SemanticCutEvidenceV1,
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
use std::time::{SystemTime, UNIX_EPOCH};
use zero_abi::{
    canonical_json, raw_worker::EffectClass, reasoning_contract_digest_v1,
    verify_strict_no_downshift_v1, zbf_contract_digest_v1, ArtifactOwnerV1,
    DigestV1 as AbiDigestV1, DurableProfileV1, ReasoningContractV1, RobustSnapCertificate,
    SnapLevel, StrictReasoningAdmissionRecordV1, StrictReasoningAdmissionV1, ZbfArtifactKindV1,
    ZbfObjectV1,
};
use zero_cert::{effect_witness_contract_digest_v1, EffectAcceptedV1, VerifiedEvidence};

/// Schema version. Bumped to 6 when the permit lease binding (expiry
/// deadline, epoch, caller session id) entered the admission/permit digests
/// (ZS-VERIFY-005).
pub const TWO_PHASE_SCHEMA_VERSION: u16 = 6;
pub const GUARD_COUNT: usize = 10;
pub const MAX_SOURCE_REPOSITORIES: usize = 64;
pub const MAX_CONTROLLER_INSTRUCTIONS: usize = 4_096;

pub type DigestV1 = [u8; 32];

const TWO_PHASE_CONTRACT_DOMAIN_V2: &[u8] = b"zerostack.kernel.contract.v2\0";
const TWO_PHASE_CONTRACT_DOMAIN_V3: &[u8] = b"zerostack.kernel.contract.v3\0";
const TWO_PHASE_CONTRACT_DOMAIN_V4: &[u8] = b"zerostack.kernel.contract.v4\0";
const TWO_PHASE_CONTRACT_DOMAIN_V5: &[u8] = b"zerostack.kernel.contract.v5\0";

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
        "contract_version": 3,
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

pub fn two_phase_contract_manifest_v4() -> Value {
    json!({
        "artifact_profile": "zbf_1_portable_strict",
        "candidate_protocol_identity": [
            "assembly_manifest_digest",
            "source_tree_digest",
            "image_digest",
            "fixed_model_digest",
            "reasoning_contract_digest",
            "two_phase_contract_digest_v4",
        ],
        "contract_version": 4,
        "guard_order": Guard::ALL,
        "linked_contracts": {
            "effect_witness": effect_witness_contract_digest_v1(),
            "quality_envelope": quality_envelope_contract_digest_v1(),
            "reasoning_contract": reasoning_contract_digest_v1(),
            "semantic_cut": semantic_cut_contract_digest_v1(),
            "transaction": transaction_contract_digest_v1(),
            "zbf": zbf_contract_digest_v1(),
        },
        "name": "zerostack.two_phase_kernel.v4",
        "negative_space": [
            "approximate_continuation_as_exact",
            "clean_restart_as_exact_continuation",
            "individual_candidate_claim_from_distributional_evidence",
            "native_filesystem_durability",
            "production_worker_contract_enforcement",
            "semantic_claim_without_verified_exact_payload",
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
            "baseline_reasoning_contract",
            "reasoning_contract",
            "baseline_reasoning_contract_digest",
            "reasoning_contract_digest",
            "reasoning_admission",
            "comparison_identity_digest",
            "semantic_cut_verifier_identity_digest",
            "artifact_set_digest",
            "semantic_cut_certificate",
            "terminal_rcq_identity_digest",
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
        "semantic_cut_admission": "exact_rcq_identity_plus_verified_canonical_claim",
        "transaction_closure": "validated_zero_gate_transaction_receipt_only",
    })
}

pub fn two_phase_contract_digest_v4() -> DigestV1 {
    let canonical = canonical_json(&two_phase_contract_manifest_v4());
    let mut bytes = Vec::with_capacity(TWO_PHASE_CONTRACT_DOMAIN_V4.len() + canonical.len());
    bytes.extend_from_slice(TWO_PHASE_CONTRACT_DOMAIN_V4);
    bytes.extend_from_slice(canonical.as_bytes());
    hash_bytes(&bytes)
}

pub fn two_phase_contract_manifest_v5() -> Value {
    json!({
        "artifact_profile": "zbf_1_portable_strict",
        "candidate_protocol_identity": [
            "assembly_manifest_digest",
            "source_tree_digest",
            "image_digest",
            "fixed_model_digest",
            "reasoning_contract_digest",
            "two_phase_contract_digest_v5",
        ],
        "contract_version": TWO_PHASE_SCHEMA_VERSION,
        "guard_order": Guard::ALL,
        "linked_contracts": {
            "deoptimization": deoptimization_contract_digest_v1(),
            "effect_witness": effect_witness_contract_digest_v1(),
            "quality_envelope": quality_envelope_contract_digest_v1(),
            "reasoning_contract": reasoning_contract_digest_v1(),
            "semantic_cut": semantic_cut_contract_digest_v1(),
            "transaction": transaction_contract_digest_v1(),
            "zbf": zbf_contract_digest_v1(),
        },
        "name": "zerostack.two_phase_kernel.v5",
        "negative_space": [
            "approximate_continuation_as_exact",
            "journal_root_recovery_as_exact_deoptimization",
            "clean_restart_as_exact_continuation",
            "cross_execution_deoptimization_receipt_replay",
            "individual_candidate_claim_from_distributional_evidence",
            "native_filesystem_durability",
            "production_worker_contract_enforcement",
            "resume_permit_as_baseline_execution_or_publication",
            "semantic_claim_without_verified_exact_payload",
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
            "baseline_reasoning_contract",
            "reasoning_contract",
            "baseline_reasoning_contract_digest",
            "reasoning_contract_digest",
            "reasoning_admission",
            "comparison_identity_digest",
            "semantic_cut_verifier_identity_digest",
            "artifact_set_digest",
            "semantic_cut_certificate",
            "terminal_rcq_identity_digest",
            "snap_certificate_digest",
            "safety_shield_digest",
            "quality_admission",
            "final_quality_selection",
            "transaction_receipt_digest",
            "deoptimization_execution_receipt_digest",
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
        "semantic_cut_admission": "exact_rcq_identity_plus_verified_canonical_claim",
        "transaction_closure": "candidate_commit_from_validated_transaction_receipt; fallback_from_verified_exact_raw_baseline_execution_receipt",
    })
}

pub fn two_phase_contract_digest_v5() -> DigestV1 {
    let canonical = canonical_json(&two_phase_contract_manifest_v5());
    let mut bytes = Vec::with_capacity(TWO_PHASE_CONTRACT_DOMAIN_V5.len() + canonical.len());
    bytes.extend_from_slice(TWO_PHASE_CONTRACT_DOMAIN_V5);
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
    ReasoningContractMismatch,
    SemanticCutCrossing,
    AttributionChanged,
    UnboundedWorker,
    BoundExceeded,
    MissingSnapCertificate,
    MissingSafetyShield,
    IrreversiblePreEvidenceEffect,
    PerformanceUnknown,
    ExecuteWithoutPermit,
    ExpiredPermit,
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
            Self::ReasoningContractMismatch => "reasoning_contract_mismatch",
            Self::SemanticCutCrossing => "semantic_cut_crossing",
            Self::AttributionChanged => "attribution_changed",
            Self::UnboundedWorker => "unbounded_worker",
            Self::BoundExceeded => "bound_exceeded",
            Self::MissingSnapCertificate => "missing_snap_certificate",
            Self::MissingSafetyShield => "missing_safety_shield",
            Self::IrreversiblePreEvidenceEffect => "irreversible_pre_evidence_effect",
            Self::PerformanceUnknown => "performance_unknown",
            Self::ExecuteWithoutPermit => "execute_without_permit",
            Self::ExpiredPermit => "expired_permit",
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
        bytes.extend_from_slice(b"zerostack.kernel.trace.v5\0");
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
    pub baseline_reasoning_contract: ReasoningContractV1,
    pub reasoning_contract: ReasoningContractV1,
    pub baseline_reasoning_contract_digest: DigestV1,
    pub reasoning_contract_digest: DigestV1,
    pub comparison_identity_digest: DigestV1,
    pub semantic_cut_verifier_identity_digest: DigestV1,
    pub predecessor_receipt_head: DigestV1,
}

impl ExecutionBinding {
    pub fn digest(&self) -> DigestV1 {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"zerostack.kernel.binding.v5\0");
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
        bytes.extend_from_slice(&self.baseline_reasoning_contract_digest);
        bytes.extend_from_slice(&self.reasoning_contract_digest);
        bytes.extend_from_slice(&self.comparison_identity_digest);
        bytes.extend_from_slice(&self.semantic_cut_verifier_identity_digest);
        bytes.extend_from_slice(&self.predecessor_receipt_head);
        hash_bytes(&bytes)
    }
}

pub fn candidate_protocol_identity_v1(binding: &ExecutionBinding) -> DigestV1 {
    let mut bytes = Vec::with_capacity(32 * 6);
    bytes.extend_from_slice(&binding.assembly_manifest_digest);
    bytes.extend_from_slice(&binding.source_tree_digest);
    bytes.extend_from_slice(&binding.image_digest);
    bytes.extend_from_slice(&binding.fixed_model_digest);
    bytes.extend_from_slice(&binding.reasoning_contract_digest);
    bytes.extend_from_slice(&two_phase_contract_digest_v5());
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
        bytes.extend_from_slice(b"zerostack.kernel.plan.v5\0");
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
pub enum AttributionClass {
    Fixed,
    Changed,
}

#[allow(clippy::large_enum_variant)]
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
    pub reasoning_admission: StrictReasoningAdmissionV1,
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
    /// Permit lease: the absolute wall-clock deadline (ms since epoch) after
    /// which the permit MUST NOT start (ZS-VERIFY-005).
    pub expiry_deadline_ms: u64,
    /// Permit lease: the epoch the permit is bound to. Zero is refused.
    pub epoch: u64,
    /// Permit lease: the caller session that may start the permit. Empty is
    /// refused.
    pub caller_session_id: String,
}

impl PrepareRequest {
    /// Canonical commitment to every G0-G7 admission input.
    pub fn admission_digest(&self) -> DigestV1 {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"zerostack.kernel.admission.v6\0");
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

        bytes.extend_from_slice(&self.expiry_deadline_ms.to_be_bytes());
        bytes.extend_from_slice(&self.epoch.to_be_bytes());
        bytes.extend_from_slice(&(self.caller_session_id.len() as u64).to_be_bytes());
        bytes.extend_from_slice(self.caller_session_id.as_bytes());

        let evidence = &self.evidence;
        bytes.extend_from_slice(&evidence.artifacts.artifact_set_digest);
        bytes.extend_from_slice(evidence.reasoning_admission.digest().as_bytes());
        bytes.extend_from_slice(&evidence.semantic_cut.certificate_digest());
        bytes.extend_from_slice(&evidence.semantic_cut.verifier_identity_digest());
        bytes.extend_from_slice(&evidence.semantic_cut.terminal_rcq_identity_digest());
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
    /// Permit lease carried on the record; bound into `permit_id`.
    pub expiry_deadline_ms: u64,
    /// Permit lease carried on the record; bound into `permit_id`.
    pub epoch: u64,
    /// Permit lease carried on the record; bound into `permit_id`.
    pub caller_session_id: String,
}

/// Opaque, linear execution authority. It cannot be deserialized or cloned.
#[derive(Debug)]
pub struct ExecutionPermit {
    permit_id: DigestV1,
    request: PrepareRequest,
    trace: ExecutionTrace,
    expiry_deadline_ms: u64,
    epoch: u64,
    caller_session_id: String,
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
            expiry_deadline_ms: self.expiry_deadline_ms,
            epoch: self.epoch,
            caller_session_id: self.caller_session_id.clone(),
        }
    }
    pub fn binding(&self) -> &ExecutionBinding {
        &self.request.binding
    }
    /// The absolute wall-clock deadline (ms since epoch) of this permit.
    pub const fn expiry_deadline_ms(&self) -> u64 {
        self.expiry_deadline_ms
    }
    /// The epoch this permit is bound to.
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }
    /// The caller session this permit is bound to.
    pub fn caller_session_id(&self) -> &str {
        &self.caller_session_id
    }
    /// Start the brokered execution now. An expired permit is refused
    /// fail-loud with `ExpiredPermit` and returns no execution (ZS-VERIFY-005).
    pub fn start(self) -> Result<BrokeredExecution, KernelError> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        self.start_at(now_ms)
    }

    /// Start at an explicit wall-clock instant (deterministic for tests and
    /// replays). The deadline check is the single expiry enforcement point:
    /// an expired permit can never produce an execution or a receipt.
    pub fn start_at(self, now_ms: u64) -> Result<BrokeredExecution, KernelError> {
        if now_ms > self.expiry_deadline_ms {
            return Err(KernelError::execution(
                FailureCode::ExpiredPermit,
                format!(
                    "permit for session '{}' expired at deadline_ms={} (start now_ms={})",
                    self.caller_session_id, self.expiry_deadline_ms, now_ms
                ),
            ));
        }
        Ok(BrokeredExecution {
            permit_id: self.permit_id,
            request: self.request,
            trace: self.trace,
            expiry_deadline_ms: self.expiry_deadline_ms,
            epoch: self.epoch,
            caller_session_id: self.caller_session_id,
            next_instruction: 0,
            usage: ResourceUsage::default(),
            verification_digest: None,
            buffered_visible: Vec::new(),
            staged_effects: Vec::new(),
        })
    }
}

pub fn prepare(request: PrepareRequest) -> Result<ExecutionPermit, PrepareFailure> {
    // Permit lease is mandatory and fail-closed: a permit without a caller
    // session or epoch is not a permit (ZS-VERIFY-005).
    if request.caller_session_id.is_empty() {
        return Err(PrepareFailure {
            error: KernelError::at(
                FailureCode::MissingBinding,
                Guard::G0Canonical,
                "permit lease: caller_session_id must be nonempty",
            ),
            trace: ExecutionTrace::new(),
        });
    }
    if request.epoch == 0 {
        return Err(PrepareFailure {
            error: KernelError::at(
                FailureCode::MissingBinding,
                Guard::G0Canonical,
                "permit lease: epoch must be nonzero",
            ),
            trace: ExecutionTrace::new(),
        });
    }
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
    let expiry_deadline_ms = request.expiry_deadline_ms;
    let epoch = request.epoch;
    let caller_session_id = request.caller_session_id.clone();
    Ok(ExecutionPermit {
        permit_id,
        request,
        trace,
        expiry_deadline_ms,
        epoch,
        caller_session_id,
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
    if record.caller_session_id.is_empty() {
        return Err(KernelError::execution(
            FailureCode::ForgedPermit,
            "permit caller_session_id is empty",
        ));
    }
    if record.epoch == 0 {
        return Err(KernelError::execution(
            FailureCode::ForgedPermit,
            "permit epoch is zero",
        ));
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"zerostack.kernel.permit.v6\0");
    bytes.extend_from_slice(&record.admission_digest);
    bytes.extend_from_slice(&record.trace.digest());
    bytes.extend_from_slice(&record.expiry_deadline_ms.to_be_bytes());
    bytes.extend_from_slice(&record.epoch.to_be_bytes());
    bytes.extend_from_slice(&(record.caller_session_id.len() as u64).to_be_bytes());
    bytes.extend_from_slice(record.caller_session_id.as_bytes());
    if record.permit_id != hash_bytes(&bytes) {
        return Err(KernelError::execution(
            FailureCode::ForgedPermit,
            "permit identity does not bind its record (incl. lease)",
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
        request.binding.baseline_reasoning_contract_digest,
        request.binding.reasoning_contract_digest,
        request.binding.comparison_identity_digest,
        request.binding.semantic_cut_verifier_identity_digest,
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
    let reasoning_contract_valid = request
        .binding
        .baseline_reasoning_contract
        .validate()
        .is_ok()
        && request
            .binding
            .baseline_reasoning_contract
            .identity_digest()
            .is_ok_and(|digest| {
                *digest.as_bytes() == request.binding.baseline_reasoning_contract_digest
            })
        && request.binding.reasoning_contract.validate().is_ok()
        && request
            .binding
            .reasoning_contract
            .identity_digest()
            .is_ok_and(|digest| *digest.as_bytes() == request.binding.reasoning_contract_digest)
        && *request
            .binding
            .reasoning_contract
            .model_identity()
            .as_bytes()
            == request.binding.fixed_model_digest
        && !request.binding.reasoning_contract.allow_effort_downshift();
    let reasoning_admission = &request.evidence.reasoning_admission;
    let reasoning_admission_valid = reasoning_admission.validate().is_ok()
        && *reasoning_admission.baseline_contract_digest().as_bytes()
            == request.binding.baseline_reasoning_contract_digest
        && *reasoning_admission.candidate_contract_digest().as_bytes()
            == request.binding.reasoning_contract_digest
        && verify_strict_no_downshift_v1(
            &request.binding.baseline_reasoning_contract,
            &request.binding.reasoning_contract,
        )
        .is_ok_and(|recomputed| recomputed.record() == reasoning_admission.record());
    if !reasoning_contract_valid || !reasoning_admission_valid {
        return Err(KernelError::at(
            FailureCode::ReasoningContractMismatch,
            Guard::G0Canonical,
            "reasoning contract or strict no-downshift admission is invalid",
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
        .ok_or_else(|| {
            KernelError::at(
                FailureCode::InvalidPlan,
                Guard::G2FinitePlan,
                "plan verify instruction missing after count check",
            )
        })?;
    let visible = instructions
        .iter()
        .position(|step| matches!(step, ControllerInstruction::BufferVisible))
        .ok_or_else(|| {
            KernelError::at(
                FailureCode::InvalidPlan,
                Guard::G2FinitePlan,
                "plan buffer instruction missing after count check",
            )
        })?;
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
    cut.validate().map_err(|error| {
        KernelError::at(
            FailureCode::SemanticCutCrossing,
            Guard::G3Attribution,
            format!("semantic-cut certificate failed validation: {error}"),
        )
    })?;
    let claim = cut.claim();
    if is_zero(&cut.certificate_digest())
        || claim.input_project_control_root() != request.binding.state_snapshot_digest
        || claim.compiled_plan_digest() != request.binding.plan_digest
        || claim.fixed_model_digest() != request.binding.fixed_model_digest
        || claim.reasoning_contract_digest() != request.binding.reasoning_contract_digest
        || claim.comparison_identity_digest() != request.binding.comparison_identity_digest
        || claim.certificate_scope_digest() != request.binding.task_fingerprint_digest
        || cut.verifier_identity_digest() != request.binding.semantic_cut_verifier_identity_digest
    {
        return Err(KernelError::at(
            FailureCode::SemanticCutCrossing,
            Guard::G3Attribution,
            "semantic cut binds another input, plan, model, reasoning contract, comparison, scope, or verifier",
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
        let repository_valid = !source.repository.is_empty()
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
    bytes.extend_from_slice(b"zerostack.kernel.permit.v6\0");
    bytes.extend_from_slice(&request.admission_digest());
    bytes.extend_from_slice(&trace.digest());
    bytes.extend_from_slice(&request.expiry_deadline_ms.to_be_bytes());
    bytes.extend_from_slice(&request.epoch.to_be_bytes());
    bytes.extend_from_slice(&(request.caller_session_id.len() as u64).to_be_bytes());
    bytes.extend_from_slice(request.caller_session_id.as_bytes());
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
    expiry_deadline_ms: u64,
    epoch: u64,
    caller_session_id: String,
    next_instruction: usize,
    usage: ResourceUsage,
    verification_digest: Option<DigestV1>,
    buffered_visible: Vec<u8>,
    staged_effects: Vec<StagedEffect>,
}

impl BrokeredExecution {
    /// The absolute wall-clock deadline (ms since epoch) of the starting permit.
    pub const fn expiry_deadline_ms(&self) -> u64 {
        self.expiry_deadline_ms
    }
    /// The epoch the starting permit is bound to.
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }
    /// The caller session the starting permit is bound to.
    pub fn caller_session_id(&self) -> &str {
        &self.caller_session_id
    }

    pub fn dispatch(&mut self, owner: PeerOwner, usage: ResourceUsage) -> Result<(), KernelError> {
        self.expect(ControllerInstruction::Dispatch { owner })?; // ubs:ignore — BrokeredExecution::expect instruction matcher, not Result::expect
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
        self.expect(ControllerInstruction::DeterministicTransform)?; // ubs:ignore — BrokeredExecution::expect instruction matcher, not Result::expect
        self.advance();
        Ok(())
    }

    pub fn record_verification(&mut self, evidence_digest: DigestV1) -> Result<(), KernelError> {
        self.expect(ControllerInstruction::Verify)?; // ubs:ignore — BrokeredExecution::expect instruction matcher, not Result::expect
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
        self.expect(ControllerInstruction::StageEffect)?; // ubs:ignore — BrokeredExecution::expect instruction matcher, not Result::expect
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
        self.expect(ControllerInstruction::BufferVisible)?; // ubs:ignore — BrokeredExecution::expect instruction matcher, not Result::expect
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
        self.expect(ControllerInstruction::CloseTransaction)?; // ubs:ignore — BrokeredExecution::expect instruction matcher, not Result::expect
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
    deoptimization_execution_receipt_digest: Option<DigestV1>,
    deoptimization_kernel_binding_digest: Option<DigestV1>,
    deoptimization_kernel_admission_digest: Option<DigestV1>,
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
            TransactionDispositionV1::BaselineRootRecovered => {
                return Err(KernelError::at(
                    FailureCode::UnaccountedFallback,
                    Guard::G8TransactionClosure,
                    "baseline recovery requires a verified exact baseline execution receipt",
                ));
            }
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
            deoptimization_execution_receipt_digest: None,
            deoptimization_kernel_binding_digest: None,
            deoptimization_kernel_admission_digest: None,
            action_digest: *receipt.action_digest().as_bytes(),
            acceptance_digest: receipt.acceptance_digest().map(|digest| *digest.as_bytes()),
            baseline_state: *receipt.baseline_state().as_bytes(),
            candidate_state: *receipt.candidate_state().as_bytes(),
            restoration_scope: receipt.restoration_scope(),
            external_restoration_debt_count: receipt.external_restoration_debt_count(),
            restoration,
        })
    }

    pub fn from_baseline_execution(
        execution_receipt: BaselineExecutionReceiptV1,
    ) -> Result<Self, KernelError> {
        execution_receipt.validate().map_err(|error| {
            KernelError::at(
                FailureCode::UnaccountedFallback,
                Guard::G8TransactionClosure,
                format!("baseline execution receipt failed validation: {error}"),
            )
        })?;
        let execution_receipt_digest = *execution_receipt.receipt_digest().as_bytes();
        let baseline_successor_root = *execution_receipt.baseline_successor_root().as_bytes();
        let baseline_transaction_receipt_digest = *execution_receipt
            .baseline_transaction_receipt_digest()
            .as_bytes();
        let baseline_action_digest = *execution_receipt.baseline_action_digest().as_bytes();
        let baseline_acceptance_digest = *execution_receipt.baseline_acceptance_digest().as_bytes();
        let kernel_binding_digest = *execution_receipt.kernel_binding_digest().as_bytes();
        let kernel_admission_digest = *execution_receipt.kernel_admission_digest().as_bytes();
        let restored = execution_receipt.restored_transaction();
        if restored.resource_count == 0 {
            return Err(KernelError::at(
                FailureCode::IncompleteTransactionClosure,
                Guard::G8TransactionClosure,
                "deoptimization receipt restored no preregistered resources",
            ));
        }
        Ok(Self {
            kind: ClosureKind::Fallback,
            root: baseline_successor_root,
            transaction_receipt_digest: baseline_transaction_receipt_digest,
            deoptimization_execution_receipt_digest: Some(execution_receipt_digest),
            deoptimization_kernel_binding_digest: Some(kernel_binding_digest),
            deoptimization_kernel_admission_digest: Some(kernel_admission_digest),
            action_digest: baseline_action_digest,
            acceptance_digest: Some(baseline_acceptance_digest),
            baseline_state: *restored.baseline_state.as_bytes(),
            candidate_state: *restored.candidate_state.as_bytes(),
            restoration_scope: RestorationScopeV1::DeclaredEffectClosure,
            external_restoration_debt_count: 0,
            restoration: RestorationAccounting {
                attempted: u64::from(restored.resource_count),
                completed: u64::from(restored.resource_count),
                debt: 0,
            },
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
        || closure
            .deoptimization_execution_receipt_digest
            .is_some_and(|digest| is_zero(&digest))
        || closure
            .deoptimization_kernel_binding_digest
            .is_some_and(|digest| is_zero(&digest))
        || closure
            .deoptimization_kernel_admission_digest
            .is_some_and(|digest| is_zero(&digest))
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
                || closure.deoptimization_execution_receipt_digest.is_some()
                || closure.deoptimization_kernel_binding_digest.is_some()
                || closure.deoptimization_kernel_admission_digest.is_some()
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
            if closure.deoptimization_execution_receipt_digest.is_none()
                || closure.deoptimization_kernel_binding_digest
                    != Some(execution.request.binding.digest())
                || closure.deoptimization_kernel_admission_digest
                    != Some(execution.request.admission_digest())
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
    pub baseline_reasoning_contract: ReasoningContractV1,
    pub reasoning_contract: ReasoningContractV1,
    pub baseline_reasoning_contract_digest: DigestV1,
    pub reasoning_contract_digest: DigestV1,
    pub reasoning_admission: StrictReasoningAdmissionRecordV1,
    pub comparison_identity_digest: DigestV1,
    pub semantic_cut_verifier_identity_digest: DigestV1,
    pub artifact_set_digest: DigestV1,
    pub semantic_cut_certificate_digest: DigestV1,
    pub semantic_cut: SemanticCutCertificateRecordV1,
    pub terminal_rcq_identity_digest: DigestV1,
    pub snap_certificate_digest: Option<DigestV1>,
    pub safety_shield_digest: DigestV1,
    pub quality_admission: QualityAdmissionRecordV1,
    pub final_quality_selection: QualitySelectionV1,
    pub transaction_receipt_digest: DigestV1,
    pub deoptimization_execution_receipt_digest: Option<DigestV1>,
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
        baseline_reasoning_contract: record.baseline_reasoning_contract.clone(),
        reasoning_contract: record.reasoning_contract.clone(),
        baseline_reasoning_contract_digest: record.baseline_reasoning_contract_digest,
        reasoning_contract_digest: record.reasoning_contract_digest,
        comparison_identity_digest: record.comparison_identity_digest,
        semantic_cut_verifier_identity_digest: record.semantic_cut_verifier_identity_digest,
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
        record.baseline_reasoning_contract_digest,
        record.reasoning_contract_digest,
        *record.reasoning_admission.admission_digest.as_bytes(),
        record.comparison_identity_digest,
        record.semantic_cut_verifier_identity_digest,
        record.artifact_set_digest,
        record.semantic_cut_certificate_digest,
        record.terminal_rcq_identity_digest,
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
        || record
            .deoptimization_execution_receipt_digest
            .is_some_and(|digest| is_zero(&digest))
        || binding.digest() != record.binding_digest
        || record.attribution_class != AttributionClass::Fixed
        || envelope_has_zero(record.resource_envelope)
        || !reasoning_receipt_fields_valid(record)
        || !semantic_cut_receipt_fields_valid(record)
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
            record.deoptimization_execution_receipt_digest.is_none()
                && record.failure_code.is_none()
                && record.restoration == RestorationAccounting::default()
        }
        ReceiptKind::Fallback => {
            record.deoptimization_execution_receipt_digest.is_some()
                && record.restoration.debt == 0
                && accounted == Some(record.restoration.attempted)
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
        *record.reasoning_admission.admission_digest.as_bytes(),
        record.semantic_cut_certificate_digest,
        record.terminal_rcq_identity_digest,
        record.snap_certificate_digest,
        record.safety_shield_digest,
        &record.quality_admission,
        record.final_quality_selection,
        record.transaction_receipt_digest,
        record.deoptimization_execution_receipt_digest,
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

fn reasoning_receipt_fields_valid(record: &ReceiptRecord) -> bool {
    record.baseline_reasoning_contract.validate().is_ok()
        && record
            .baseline_reasoning_contract
            .identity_digest()
            .is_ok_and(|digest| *digest.as_bytes() == record.baseline_reasoning_contract_digest)
        && record.reasoning_contract.validate().is_ok()
        && record
            .reasoning_contract
            .identity_digest()
            .is_ok_and(|digest| *digest.as_bytes() == record.reasoning_contract_digest)
        && *record.reasoning_contract.model_identity().as_bytes() == record.fixed_model_digest
        && !record.reasoning_contract.allow_effort_downshift()
        && record.reasoning_admission.validate().is_ok()
        && *record
            .reasoning_admission
            .baseline_contract_digest
            .as_bytes()
            == record.baseline_reasoning_contract_digest
        && *record
            .reasoning_admission
            .candidate_contract_digest
            .as_bytes()
            == record.reasoning_contract_digest
        && verify_strict_no_downshift_v1(
            &record.baseline_reasoning_contract,
            &record.reasoning_contract,
        )
        .is_ok_and(|recomputed| recomputed.record() == record.reasoning_admission)
}

fn semantic_cut_receipt_fields_valid(record: &ReceiptRecord) -> bool {
    let claim = &record.semantic_cut.claim;
    record.semantic_cut.validate().is_ok()
        && record.semantic_cut.certificate_digest == record.semantic_cut_certificate_digest
        && record.semantic_cut.verifier_identity_digest
            == record.semantic_cut_verifier_identity_digest
        && claim.terminal_rcq_identity_digest() == record.terminal_rcq_identity_digest
        && claim.input_project_control_root() == record.state_snapshot_digest
        && claim.compiled_plan_digest() == record.plan_digest
        && claim.fixed_model_digest() == record.fixed_model_digest
        && claim.reasoning_contract_digest() == record.reasoning_contract_digest
        && claim.comparison_identity_digest() == record.comparison_identity_digest
        && claim.certificate_scope_digest() == record.task_fingerprint_digest
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
                baseline_reasoning_contract: record.baseline_reasoning_contract.clone(),
                reasoning_contract: record.reasoning_contract.clone(),
                baseline_reasoning_contract_digest: record.baseline_reasoning_contract_digest,
                reasoning_contract_digest: record.reasoning_contract_digest,
                comparison_identity_digest: record.comparison_identity_digest,
                semantic_cut_verifier_identity_digest: record.semantic_cut_verifier_identity_digest,
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
        let reasoning_admission = self.request.evidence.reasoning_admission.record();
        let semantic_cut_certificate_digest =
            self.request.evidence.semantic_cut.certificate_digest();
        let semantic_cut = self.request.evidence.semantic_cut.record();
        let terminal_rcq_identity_digest = self
            .request
            .evidence
            .semantic_cut
            .terminal_rcq_identity_digest();
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
        let deoptimization_execution_receipt_digest =
            self.closure.deoptimization_execution_receipt_digest;
        let attribution_class = AttributionClass::Fixed;
        let effect_class = self.request.effect_class;
        let resource_envelope = self.request.envelope;
        let receipt_head = receipt_digest(
            kind,
            self.permit_id,
            &self.request.binding,
            admission_digest,
            artifact_set_digest,
            *reasoning_admission.admission_digest.as_bytes(),
            semantic_cut_certificate_digest,
            terminal_rcq_identity_digest,
            snap_certificate_digest,
            safety_shield_digest,
            &quality_admission,
            final_quality_selection,
            transaction_receipt_digest,
            deoptimization_execution_receipt_digest,
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
            reasoning_admission,
            semantic_cut_certificate_digest,
            semantic_cut,
            terminal_rcq_identity_digest,
            snap_certificate_digest,
            safety_shield_digest,
            quality_admission,
            final_quality_selection,
            transaction_receipt_digest,
            deoptimization_execution_receipt_digest,
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
    reasoning_admission: StrictReasoningAdmissionRecordV1,
    semantic_cut_certificate_digest: DigestV1,
    semantic_cut: SemanticCutCertificateRecordV1,
    terminal_rcq_identity_digest: DigestV1,
    snap_certificate_digest: Option<DigestV1>,
    safety_shield_digest: DigestV1,
    quality_admission: QualityAdmissionRecordV1,
    final_quality_selection: QualitySelectionV1,
    transaction_receipt_digest: DigestV1,
    deoptimization_execution_receipt_digest: Option<DigestV1>,
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
            baseline_reasoning_contract: self.binding.baseline_reasoning_contract.clone(),
            reasoning_contract: self.binding.reasoning_contract.clone(),
            baseline_reasoning_contract_digest: self.binding.baseline_reasoning_contract_digest,
            reasoning_contract_digest: self.binding.reasoning_contract_digest,
            reasoning_admission: self.reasoning_admission.clone(),
            comparison_identity_digest: self.binding.comparison_identity_digest,
            semantic_cut_verifier_identity_digest: self
                .binding
                .semantic_cut_verifier_identity_digest,
            artifact_set_digest: self.artifact_set_digest,
            semantic_cut_certificate_digest: self.semantic_cut_certificate_digest,
            semantic_cut: self.semantic_cut.clone(),
            terminal_rcq_identity_digest: self.terminal_rcq_identity_digest,
            snap_certificate_digest: self.snap_certificate_digest,
            safety_shield_digest: self.safety_shield_digest,
            quality_admission: self.quality_admission.clone(),
            final_quality_selection: self.final_quality_selection,
            transaction_receipt_digest: self.transaction_receipt_digest,
            deoptimization_execution_receipt_digest: self.deoptimization_execution_receipt_digest,
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
    reasoning_admission_digest: DigestV1,
    semantic_cut_certificate_digest: DigestV1,
    terminal_rcq_identity_digest: DigestV1,
    snap_certificate_digest: Option<DigestV1>,
    safety_shield_digest: DigestV1,
    quality_admission: &QualityAdmissionRecordV1,
    final_quality_selection: QualitySelectionV1,
    transaction_receipt_digest: DigestV1,
    deoptimization_execution_receipt_digest: Option<DigestV1>,
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
    bytes.extend_from_slice(b"zerostack.kernel.receipt.v5\0");
    bytes.extend_from_slice(&TWO_PHASE_SCHEMA_VERSION.to_be_bytes());
    bytes.push(kind as u8);
    bytes.extend_from_slice(&permit_id);
    bytes.extend_from_slice(&binding.digest());
    bytes.extend_from_slice(&admission_digest);
    bytes.extend_from_slice(&artifact_set_digest);
    bytes.extend_from_slice(&reasoning_admission_digest);
    bytes.extend_from_slice(&semantic_cut_certificate_digest);
    bytes.extend_from_slice(&terminal_rcq_identity_digest);
    append_optional_digest(&mut bytes, snap_certificate_digest);
    bytes.extend_from_slice(&safety_shield_digest);
    bytes.extend_from_slice(quality_admission.admission_digest.as_bytes());
    bytes.push(match final_quality_selection {
        QualitySelectionV1::Candidate => 0,
        QualitySelectionV1::FrozenBaseline => 1,
    });
    bytes.extend_from_slice(&transaction_receipt_digest);
    append_optional_digest(&mut bytes, deoptimization_execution_receipt_digest);
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
    bytes.extend_from_slice(b"zerostack.kernel.effects.v5\0");
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

