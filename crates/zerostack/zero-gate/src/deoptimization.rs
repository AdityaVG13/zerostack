//! Proof-carrying exact deoptimization to a frozen raw-baseline safepoint. Deoptimization is a
//! first-class transition.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zero_abi::{
    NativeStatePolicy, ReasoningContract, Sha256Digest, canonical_json, reasoning_contract_digest,
    sha256,
};
use zero_cert::{CompletenessWitness, Query, VerifiedEvidence};
use zero_store::RecoveryOutcome;

use crate::{
    quality::{QualityAdmissionRecord, QualitySelection, quality_envelope_contract_digest},
    recovery::{RecoveryUnknownDecision, dcr_contract_digest},
    semantic_cut::{ReasoningSafepoint, ReasoningStateStatus},
    transaction::{
        RestorationScope, TransactionDisposition, TransactionReceipt, transaction_contract_digest,
    },
    two_phase::{FailureCode, WorkerEnvelope},
};

pub const DEOPTIMIZATION_CONTRACT_VERSION: u16 = 1;
pub const BASELINE_SAFEPOINT_SCHEMA_VERSION: &str = "zerostack.baseline_safepoint";
pub const BASELINE_RESTORATION_SCHEMA_VERSION: &str = "zerostack.baseline_restoration";
pub const BASELINE_EXECUTION_SCHEMA_VERSION: &str = "zerostack.baseline_execution";
pub const DEOPTIMIZATION_PLAN_SCHEMA_VERSION: &str = "zerostack.deoptimization_plan";
pub const DEOPTIMIZATION_MAX_CANONICAL_BYTES: usize = 1_048_576;

const SAFEPOINT_DOMAIN: &[u8] = b"zerostack.deoptimization.safepoint_claim\0";
const SAFEPOINT_CERTIFICATE_DOMAIN: &[u8] = b"zerostack.deoptimization.safepoint_certificate\0";
const REASONING_ENTRY_DOMAIN: &[u8] = b"zerostack.deoptimization.reasoning_entry\0";
const PLAN_DOMAIN: &[u8] = b"zerostack.deoptimization.plan\0";
const RESTORATION_CLAIM_DOMAIN: &[u8] = b"zerostack.deoptimization.restoration_claim\0";
const RESUME_PERMIT_DOMAIN: &[u8] = b"zerostack.deoptimization.resume_permit\0";
const INVOCATION_DOMAIN: &[u8] = b"zerostack.deoptimization.baseline_invocation\0";
const EXECUTION_CLAIM_DOMAIN: &[u8] = b"zerostack.deoptimization.baseline_execution_claim\0";
const EXECUTION_RECEIPT_DOMAIN: &[u8] = b"zerostack.deoptimization.baseline_execution_receipt\0";
const VERIFIER_DOMAIN: &[u8] = b"zerostack.deoptimization.verifier_identity\0";
const CONTRACT_DOMAIN: &[u8] = b"zerostack.deoptimization.contract\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RaccWork {
    pub logical_input_tokens: u64,
    pub uncached_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub reasoning_tokens: u64,
    pub visible_output_tokens: u64,
    pub tool_calls: u64,
    pub verifier_work: u64,
    pub fallback_work: u64,
    pub latency_micros: u64,
    pub peak_memory_bytes: u64,
}

impl RaccWork {
    fn validate_limit(&self, label: &'static str) -> Result<(), DeoptimizationError> {
        let observed_input = self
            .uncached_input_tokens
            .checked_add(self.cached_input_tokens)
            .ok_or_else(|| {
                deopt_error(
                    DeoptimizationFailureCode::InvalidResourceReserve,
                    format!("{label} input-token reserve overflows"),
                )
            })?;
        if observed_input != self.logical_input_tokens
            || self.fallback_work == 0
            || self.latency_micros == 0
            || self.peak_memory_bytes == 0
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::InvalidResourceReserve,
                format!(
                    "{label} must conserve input tokens and reserve positive fallback work, latency, and peak memory"
                ),
            ));
        }
        Ok(())
    }

    fn validate_usage(&self) -> Result<(), DeoptimizationError> {
        let observed_input = self
            .uncached_input_tokens
            .checked_add(self.cached_input_tokens)
            .ok_or_else(|| {
                deopt_error(
                    DeoptimizationFailureCode::InvalidResourceUsage,
                    "input-token usage overflows",
                )
            })?;
        if observed_input != self.logical_input_tokens
            || self.fallback_work == 0
            || self.latency_micros == 0
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::InvalidResourceUsage,
                "deoptimization usage must conserve input tokens and charge fallback work and latency",
            ));
        }
        Ok(())
    }

    fn within(&self, limit: &Self) -> bool {
        self.logical_input_tokens <= limit.logical_input_tokens
            && self.uncached_input_tokens <= limit.uncached_input_tokens
            && self.cached_input_tokens <= limit.cached_input_tokens
            && self.reasoning_tokens <= limit.reasoning_tokens
            && self.visible_output_tokens <= limit.visible_output_tokens
            && self.tool_calls <= limit.tool_calls
            && self.verifier_work <= limit.verifier_work
            && self.fallback_work <= limit.fallback_work
            && self.latency_micros <= limit.latency_micros
            && self.peak_memory_bytes <= limit.peak_memory_bytes
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RouteUsage {
    pub fuel: u64,
    pub elapsed_ms: u64,
    pub io_bytes: u64,
    pub output_bytes: u64,
    pub memory_bytes: u64,
    pub processes: u32,
    pub risk_units: u64,
    pub worker_steps: u64,
    pub work: RaccWork,
}

impl RouteUsage {
    fn validate(&self) -> Result<(), DeoptimizationError> {
        self.work.validate_usage()?;
        if self.elapsed_ms == 0 || self.memory_bytes == 0 || self.worker_steps == 0 {
            return Err(deopt_error(
                DeoptimizationFailureCode::InvalidResourceUsage,
                "route usage must charge elapsed time, memory, and worker steps",
            ));
        }
        Ok(())
    }

    fn within(&self, envelope: &WorkerEnvelope, work_limit: &RaccWork) -> bool {
        self.fuel <= envelope.fuel
            && self.elapsed_ms <= envelope.deadline_ms
            && self.io_bytes <= envelope.io_bytes
            && self.output_bytes <= envelope.output_bytes
            && self.memory_bytes <= envelope.memory_bytes
            && self.processes <= envelope.processes
            && self.risk_units <= envelope.risk_units
            && self.worker_steps <= envelope.worker_steps
            && self.work.within(work_limit)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FallbackReserve {
    pub deoptimization_envelope: WorkerEnvelope,
    pub raw_baseline_envelope: WorkerEnvelope,
    pub deoptimization_work_limit: RaccWork,
    pub raw_baseline_work_limit: RaccWork,
}

impl FallbackReserve {
    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        validate_envelope("deoptimization", &self.deoptimization_envelope)?;
        validate_envelope("raw baseline", &self.raw_baseline_envelope)?;
        self.deoptimization_work_limit
            .validate_limit("deoptimization work")?;
        self.raw_baseline_work_limit
            .validate_limit("raw baseline work")?;
        Ok(())
    }
}

fn validate_envelope(
    label: &'static str,
    envelope: &WorkerEnvelope,
) -> Result<(), DeoptimizationError> {
    if envelope.fuel == 0
        || envelope.deadline_ms == 0
        || envelope.io_bytes == 0
        || envelope.output_bytes == 0
        || envelope.memory_bytes == 0
        || envelope.processes == 0
        || envelope.worker_steps == 0
    {
        return Err(deopt_error(
            DeoptimizationFailureCode::InvalidResourceReserve,
            format!("{label} runtime envelope is not fully reserved"),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "entry_kind")]
pub enum BaselineReasoningEntry {
    ExactNativeContinuation {
        opaque_state_digest: Sha256Digest,
        parent_response_digest: Sha256Digest,
        session_identity_digest: Sha256Digest,
    },
    CanonicalCleanStart {
        clean_start_identity_digest: Sha256Digest,
    },
}

impl BaselineReasoningEntry {
    fn validate(
        &self,
        contract: &ReasoningContract,
        safepoint: &ReasoningSafepoint,
    ) -> Result<(), DeoptimizationError> {
        match self {
            Self::ExactNativeContinuation {
                opaque_state_digest,
                parent_response_digest,
                session_identity_digest,
            } => {
                require_nonzero(
                    "exact native baseline entry",
                    &[
                        *opaque_state_digest,
                        *parent_response_digest,
                        *session_identity_digest,
                    ],
                )?;
                if !matches!(
                    contract.native_state_policy(),
                    NativeStatePolicy::ExactRequired | NativeStatePolicy::ExactIfAvailable
                ) || safepoint.reasoning_state_status() != ReasoningStateStatus::ExactPreserved
                    || safepoint.opaque_reasoning_state_digest() != *opaque_state_digest.as_bytes()
                {
                    return Err(deopt_error(
                        DeoptimizationFailureCode::ReasoningEntryMismatch,
                        "native continuation does not match the frozen contract and safepoint",
                    ));
                }
            }
            Self::CanonicalCleanStart {
                clean_start_identity_digest,
            } => {
                require_nonzero(
                    "clean-start baseline entry",
                    &[*clean_start_identity_digest],
                )?;
                if contract.native_state_policy() != NativeStatePolicy::CleanRestart
                    || safepoint.reasoning_state_status() != ReasoningStateStatus::ExactCleanRestart
                    || safepoint.opaque_reasoning_state_digest()
                        != *clean_start_identity_digest.as_bytes()
                {
                    return Err(deopt_error(
                        DeoptimizationFailureCode::ReasoningEntryMismatch,
                        "clean-start identity does not match the frozen baseline contract",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<Sha256Digest, DeoptimizationError> {
        Ok(domain_digest(
            REASONING_ENTRY_DOMAIN,
            &canonical_bytes(self)?,
        ))
    }
}

/// Full frozen baseline state. This claim is data until exact verified bytes
/// mint `BaselineSafepointEvidence` before candidate execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineSafepointClaim {
    schema_version: String,
    project_snapshot_root: Sha256Digest,
    working_tree_scope_digest: Sha256Digest,
    external_state_inventory_digest: Sha256Digest,
    comparison_identity_digest: Sha256Digest,
    raw_baseline_identity_digest: Sha256Digest,
    raw_baseline_input_digest: Sha256Digest,
    raw_decision_view_digest: Sha256Digest,
    assembly_contract_digest: Sha256Digest,
    baseline_engine_contract_digest: Sha256Digest,
    effect_schema_digest: Sha256Digest,
    baseline_reasoning_contract: ReasoningContract,
    baseline_reasoning_contract_digest: Sha256Digest,
    reasoning_safepoint: ReasoningSafepoint,
    reasoning_entry: BaselineReasoningEntry,
    sampler_randomness_identity_digest: Sha256Digest,
    baseline_verifier_identity_digest: Sha256Digest,
    reserve: FallbackReserve,
    transaction_route_digest: Sha256Digest,
    restoration_route_digest: Sha256Digest,
    capture_receipt_head_digest: Sha256Digest,
}

impl BaselineSafepointClaim {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_snapshot_root: Sha256Digest,
        working_tree_scope_digest: Sha256Digest,
        external_state_inventory_digest: Sha256Digest,
        comparison_identity_digest: Sha256Digest,
        raw_baseline_identity_digest: Sha256Digest,
        raw_baseline_input_digest: Sha256Digest,
        raw_decision_view_digest: Sha256Digest,
        assembly_contract_digest: Sha256Digest,
        baseline_engine_contract_digest: Sha256Digest,
        effect_schema_digest: Sha256Digest,
        baseline_reasoning_contract: ReasoningContract,
        reasoning_safepoint: ReasoningSafepoint,
        reasoning_entry: BaselineReasoningEntry,
        sampler_randomness_identity_digest: Sha256Digest,
        baseline_verifier_identity_digest: Sha256Digest,
        reserve: FallbackReserve,
        transaction_route_digest: Sha256Digest,
        restoration_route_digest: Sha256Digest,
        capture_receipt_head_digest: Sha256Digest,
    ) -> Result<Self, DeoptimizationError> {
        let baseline_reasoning_contract_digest = baseline_reasoning_contract
            .identity_digest()
            .map_err(|error| reasoning_error(error.to_string()))?;
        let claim = Self {
            schema_version: BASELINE_SAFEPOINT_SCHEMA_VERSION.into(),
            project_snapshot_root,
            working_tree_scope_digest,
            external_state_inventory_digest,
            comparison_identity_digest,
            raw_baseline_identity_digest,
            raw_baseline_input_digest,
            raw_decision_view_digest,
            assembly_contract_digest,
            baseline_engine_contract_digest,
            effect_schema_digest,
            baseline_reasoning_contract,
            baseline_reasoning_contract_digest,
            reasoning_safepoint,
            reasoning_entry,
            sampler_randomness_identity_digest,
            baseline_verifier_identity_digest,
            reserve,
            transaction_route_digest,
            restoration_route_digest,
            capture_receipt_head_digest,
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        if self.schema_version != BASELINE_SAFEPOINT_SCHEMA_VERSION {
            return Err(deopt_error(
                DeoptimizationFailureCode::SchemaVersionMismatch,
                "baseline safepoint schema is unsupported",
            ));
        }
        self.baseline_reasoning_contract
            .validate()
            .map_err(|error| reasoning_error(error.to_string()))?;
        self.reasoning_safepoint
            .validate()
            .map_err(|error| reasoning_error(error.to_string()))?;
        self.reserve.validate()?;
        require_nonzero(
            "baseline safepoint",
            &[
                self.project_snapshot_root,
                self.working_tree_scope_digest,
                self.external_state_inventory_digest,
                self.comparison_identity_digest,
                self.raw_baseline_identity_digest,
                self.raw_baseline_input_digest,
                self.raw_decision_view_digest,
                self.assembly_contract_digest,
                self.baseline_engine_contract_digest,
                self.effect_schema_digest,
                self.baseline_reasoning_contract_digest,
                self.sampler_randomness_identity_digest,
                self.baseline_verifier_identity_digest,
                self.transaction_route_digest,
                self.restoration_route_digest,
                self.capture_receipt_head_digest,
            ],
        )?;
        let actual_reasoning_digest = self
            .baseline_reasoning_contract
            .identity_digest()
            .map_err(|error| reasoning_error(error.to_string()))?;
        if actual_reasoning_digest != self.baseline_reasoning_contract_digest
            || self.reasoning_safepoint.reasoning_contract_digest()
                != *self.baseline_reasoning_contract_digest.as_bytes()
            || self.reasoning_safepoint.fixed_model_digest()
                != *self.baseline_reasoning_contract.model_identity().as_bytes()
            || self.reasoning_safepoint.project_control_root()
                != *self.project_snapshot_root.as_bytes()
            || self.reasoning_safepoint.receipt_head_digest()
                != *self.capture_receipt_head_digest.as_bytes()
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::SafepointBindingMismatch,
                "project, model, reasoning contract, or receipt head differs at safepoint",
            ));
        }
        self.reasoning_entry
            .validate(&self.baseline_reasoning_contract, &self.reasoning_safepoint)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DeoptimizationError> {
        let claim: Self = decode_canonical(bytes)?;
        claim.validate()?;
        Ok(claim)
    }

    pub fn digest(&self) -> Result<Sha256Digest, DeoptimizationError> {
        Ok(domain_digest(SAFEPOINT_DOMAIN, &self.canonical_bytes()?))
    }
}

/// Opaque capture authority. Successful exact verifier output must predate the
/// candidate plan through the frozen receipt-head binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineSafepointEvidence {
    contract_version: u16,
    claim: BaselineSafepointClaim,
    claim_digest: Sha256Digest,
    evidence_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
    certificate_digest: Sha256Digest,
}

impl BaselineSafepointEvidence {
    pub fn verify_owner_scoped(
        claim: BaselineSafepointClaim,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, DeoptimizationError> {
        claim.validate()?;
        verify_exact_successful_payload(&claim.canonical_bytes()?, evidence)?;
        let verifier_identity_digest = deoptimization_verifier_identity(evidence);
        if verifier_identity_digest != claim.baseline_verifier_identity_digest {
            return Err(deopt_error(
                DeoptimizationFailureCode::VerifierIdentityMismatch,
                "safepoint verifier differs from the frozen verifier route",
            ));
        }
        let claim_digest = claim.digest()?;
        let evidence_digest = verified_evidence_digest(evidence)?;
        let certificate_digest = digest_value(
            SAFEPOINT_CERTIFICATE_DOMAIN,
            &json!({
                "claim_digest": claim_digest,
                "contract_version": DEOPTIMIZATION_CONTRACT_VERSION,
                "evidence_digest": evidence_digest,
                "verifier_identity_digest": verifier_identity_digest,
            }),
        );
        Ok(Self {
            contract_version: DEOPTIMIZATION_CONTRACT_VERSION,
            claim,
            claim_digest,
            evidence_digest,
            verifier_identity_digest,
            certificate_digest,
        })
    }

    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION {
            return Err(deopt_error(
                DeoptimizationFailureCode::SchemaVersionMismatch,
                "safepoint certificate contract is unsupported",
            ));
        }
        self.claim.validate()?;
        require_nonzero(
            "safepoint proof",
            &[
                self.evidence_digest,
                self.verifier_identity_digest,
                self.certificate_digest,
            ],
        )?;
        let expected = digest_value(
            SAFEPOINT_CERTIFICATE_DOMAIN,
            &json!({
                "claim_digest": self.claim_digest,
                "contract_version": self.contract_version,
                "evidence_digest": self.evidence_digest,
                "verifier_identity_digest": self.verifier_identity_digest,
            }),
        );
        if self.claim.digest()? != self.claim_digest
            || self.verifier_identity_digest != self.claim.baseline_verifier_identity_digest
            || expected != self.certificate_digest
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::CertificateDigestMismatch,
                "safepoint certificate does not bind its claim and proof",
            ));
        }
        Ok(())
    }

    pub fn record(&self) -> BaselineSafepointCertificateRecord {
        BaselineSafepointCertificateRecord {
            contract_version: self.contract_version,
            claim: self.claim.clone(),
            claim_digest: self.claim_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            certificate_digest: self.certificate_digest,
        }
    }

    pub const fn claim(&self) -> &BaselineSafepointClaim {
        &self.claim
    }
    pub const fn certificate_digest(&self) -> Sha256Digest {
        self.certificate_digest
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineSafepointCertificateRecord {
    pub contract_version: u16,
    pub claim: BaselineSafepointClaim,
    pub claim_digest: Sha256Digest,
    pub evidence_digest: Sha256Digest,
    pub verifier_identity_digest: Sha256Digest,
    pub certificate_digest: Sha256Digest,
}

impl BaselineSafepointCertificateRecord {
    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        BaselineSafepointEvidence {
            contract_version: self.contract_version,
            claim: self.claim.clone(),
            claim_digest: self.claim_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            certificate_digest: self.certificate_digest,
        }
        .validate()
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationError> {
        self.validate()?;
        canonical_bytes(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "trigger_kind")]
pub enum DeoptimizationTrigger {
    RecoveryUnknown {
        problem_digest: Sha256Digest,
        decision_digest: Sha256Digest,
    },
    QualityBaselineSelection {
        quality_admission_digest: Sha256Digest,
    },
    FailClosed {
        failure_code: FailureCode,
        failure_receipt_digest: Sha256Digest,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeoptimizationPlanClaim {
    schema_version: String,
    safepoint_certificate_digest: Sha256Digest,
    trigger: DeoptimizationTrigger,
    candidate_action_digest: Sha256Digest,
    candidate_state_digest: Sha256Digest,
    candidate_closure_manifest_digest: Sha256Digest,
    prior_work_receipt_digest: Sha256Digest,
    kernel_binding_digest: Sha256Digest,
    kernel_admission_digest: Sha256Digest,
    plan_digest: Sha256Digest,
}

impl DeoptimizationPlanClaim {
    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        if self.schema_version != DEOPTIMIZATION_PLAN_SCHEMA_VERSION {
            return Err(deopt_error(
                DeoptimizationFailureCode::SchemaVersionMismatch,
                "deoptimization plan claim schema is unsupported",
            ));
        }
        require_nonzero(
            "deoptimization plan claim",
            &[
                self.safepoint_certificate_digest,
                self.candidate_action_digest,
                self.candidate_state_digest,
                self.candidate_closure_manifest_digest,
                self.prior_work_receipt_digest,
                self.kernel_binding_digest,
                self.kernel_admission_digest,
                self.plan_digest,
            ],
        )?;
        let expected = digest_value(
            PLAN_DOMAIN,
            &json!({
                "candidate_action_digest": self.candidate_action_digest,
                "candidate_closure_manifest_digest": self.candidate_closure_manifest_digest,
                "candidate_state_digest": self.candidate_state_digest,
                "contract_version": DEOPTIMIZATION_CONTRACT_VERSION,
                "kernel_admission_digest": self.kernel_admission_digest,
                "kernel_binding_digest": self.kernel_binding_digest,
                "prior_work_receipt_digest": self.prior_work_receipt_digest,
                "safepoint_certificate_digest": self.safepoint_certificate_digest,
                "trigger": self.trigger,
            }),
        );
        if expected != self.plan_digest {
            return Err(deopt_error(
                DeoptimizationFailureCode::PlanDigestMismatch,
                "deoptimization plan claim does not replay",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DeoptimizationError> {
        let claim: Self = decode_canonical(bytes)?;
        claim.validate()?;
        Ok(claim)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeoptimizationPlan {
    contract_version: u16,
    safepoint: BaselineSafepointEvidence,
    trigger: DeoptimizationTrigger,
    candidate_action_digest: Sha256Digest,
    candidate_state_digest: Sha256Digest,
    candidate_closure_manifest_digest: Sha256Digest,
    prior_work_receipt_digest: Sha256Digest,
    kernel_binding_digest: Sha256Digest,
    kernel_admission_digest: Sha256Digest,
    plan_digest: Sha256Digest,
}

impl DeoptimizationPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn for_recovery_unknown(
        safepoint: BaselineSafepointEvidence,
        unknown: &RecoveryUnknownDecision,
        candidate_action_digest: Sha256Digest,
        candidate_state_digest: Sha256Digest,
        candidate_closure_manifest_digest: Sha256Digest,
        prior_work_receipt_digest: Sha256Digest,
        kernel_binding_digest: Sha256Digest,
        kernel_admission_digest: Sha256Digest,
    ) -> Result<Self, DeoptimizationError> {
        if !unknown.raw_baseline_required()
            || unknown.fallback_safepoint() != safepoint.certificate_digest()
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::TriggerMismatch,
                "DCR Unknown does not require this frozen safepoint",
            ));
        }
        Self::new(
            safepoint,
            DeoptimizationTrigger::RecoveryUnknown {
                problem_digest: unknown.problem_digest(),
                decision_digest: unknown.decision_digest(),
            },
            candidate_action_digest,
            candidate_state_digest,
            candidate_closure_manifest_digest,
            prior_work_receipt_digest,
            kernel_binding_digest,
            kernel_admission_digest,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn for_quality_fallback(
        safepoint: BaselineSafepointEvidence,
        admission: &QualityAdmissionRecord,
        candidate_action_digest: Sha256Digest,
        candidate_state_digest: Sha256Digest,
        candidate_closure_manifest_digest: Sha256Digest,
        prior_work_receipt_digest: Sha256Digest,
        kernel_binding_digest: Sha256Digest,
        kernel_admission_digest: Sha256Digest,
    ) -> Result<Self, DeoptimizationError> {
        admission.validate().map_err(|error| {
            deopt_error(
                DeoptimizationFailureCode::TriggerMismatch,
                format!("quality admission is invalid: {error}"),
            )
        })?;
        if admission.selection != QualitySelection::FrozenBaseline
            || admission.raw_baseline_identity_digest
                != safepoint.claim.raw_baseline_identity_digest
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::TriggerMismatch,
                "quality admission does not select the frozen safepoint baseline",
            ));
        }
        Self::new(
            safepoint,
            DeoptimizationTrigger::QualityBaselineSelection {
                quality_admission_digest: admission.admission_digest,
            },
            candidate_action_digest,
            candidate_state_digest,
            candidate_closure_manifest_digest,
            prior_work_receipt_digest,
            kernel_binding_digest,
            kernel_admission_digest,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn for_fail_closed(
        safepoint: BaselineSafepointEvidence,
        failure_code: FailureCode,
        failure_receipt_digest: Sha256Digest,
        candidate_action_digest: Sha256Digest,
        candidate_state_digest: Sha256Digest,
        candidate_closure_manifest_digest: Sha256Digest,
        prior_work_receipt_digest: Sha256Digest,
        kernel_binding_digest: Sha256Digest,
        kernel_admission_digest: Sha256Digest,
    ) -> Result<Self, DeoptimizationError> {
        require_nonzero("failure receipt", &[failure_receipt_digest])?;
        Self::new(
            safepoint,
            DeoptimizationTrigger::FailClosed {
                failure_code,
                failure_receipt_digest,
            },
            candidate_action_digest,
            candidate_state_digest,
            candidate_closure_manifest_digest,
            prior_work_receipt_digest,
            kernel_binding_digest,
            kernel_admission_digest,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        safepoint: BaselineSafepointEvidence,
        trigger: DeoptimizationTrigger,
        candidate_action_digest: Sha256Digest,
        candidate_state_digest: Sha256Digest,
        candidate_closure_manifest_digest: Sha256Digest,
        prior_work_receipt_digest: Sha256Digest,
        kernel_binding_digest: Sha256Digest,
        kernel_admission_digest: Sha256Digest,
    ) -> Result<Self, DeoptimizationError> {
        safepoint.validate()?;
        require_nonzero(
            "deoptimization plan",
            &[
                candidate_action_digest,
                candidate_state_digest,
                candidate_closure_manifest_digest,
                prior_work_receipt_digest,
                kernel_binding_digest,
                kernel_admission_digest,
            ],
        )?;
        let mut plan = Self {
            contract_version: DEOPTIMIZATION_CONTRACT_VERSION,
            safepoint,
            trigger,
            candidate_action_digest,
            candidate_state_digest,
            candidate_closure_manifest_digest,
            prior_work_receipt_digest,
            kernel_binding_digest,
            kernel_admission_digest,
            plan_digest: Sha256Digest::ZERO,
        };
        plan.plan_digest = plan.expected_digest()?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION {
            return Err(deopt_error(
                DeoptimizationFailureCode::SchemaVersionMismatch,
                "deoptimization plan contract is unsupported",
            ));
        }
        self.safepoint.validate()?;
        require_nonzero(
            "deoptimization plan",
            &[
                self.candidate_action_digest,
                self.candidate_state_digest,
                self.candidate_closure_manifest_digest,
                self.prior_work_receipt_digest,
                self.kernel_binding_digest,
                self.kernel_admission_digest,
            ],
        )?;
        if self.expected_digest()? != self.plan_digest {
            return Err(deopt_error(
                DeoptimizationFailureCode::PlanDigestMismatch,
                "deoptimization plan digest does not bind its trigger and candidate attempt",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<Sha256Digest, DeoptimizationError> {
        Ok(digest_value(
            PLAN_DOMAIN,
            &json!({
                "candidate_action_digest": self.candidate_action_digest,
                "candidate_closure_manifest_digest": self.candidate_closure_manifest_digest,
                "candidate_state_digest": self.candidate_state_digest,
                "contract_version": self.contract_version,
                "kernel_admission_digest": self.kernel_admission_digest,
                "kernel_binding_digest": self.kernel_binding_digest,
                "prior_work_receipt_digest": self.prior_work_receipt_digest,
                "safepoint_certificate_digest": self.safepoint.certificate_digest,
                "trigger": self.trigger,
            }),
        ))
    }

    pub fn record(&self) -> DeoptimizationPlanRecord {
        DeoptimizationPlanRecord {
            contract_version: self.contract_version,
            safepoint: self.safepoint.record(),
            trigger: self.trigger.clone(),
            candidate_action_digest: self.candidate_action_digest,
            candidate_state_digest: self.candidate_state_digest,
            candidate_closure_manifest_digest: self.candidate_closure_manifest_digest,
            prior_work_receipt_digest: self.prior_work_receipt_digest,
            kernel_binding_digest: self.kernel_binding_digest,
            kernel_admission_digest: self.kernel_admission_digest,
            plan_digest: self.plan_digest,
        }
    }

    pub fn claim(&self) -> DeoptimizationPlanClaim {
        DeoptimizationPlanClaim {
            schema_version: DEOPTIMIZATION_PLAN_SCHEMA_VERSION.into(),
            safepoint_certificate_digest: self.safepoint.certificate_digest,
            trigger: self.trigger.clone(),
            candidate_action_digest: self.candidate_action_digest,
            candidate_state_digest: self.candidate_state_digest,
            candidate_closure_manifest_digest: self.candidate_closure_manifest_digest,
            prior_work_receipt_digest: self.prior_work_receipt_digest,
            kernel_binding_digest: self.kernel_binding_digest,
            kernel_admission_digest: self.kernel_admission_digest,
            plan_digest: self.plan_digest,
        }
    }

    pub const fn plan_digest(&self) -> Sha256Digest {
        self.plan_digest
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeoptimizationPlanRecord {
    pub contract_version: u16,
    pub safepoint: BaselineSafepointCertificateRecord,
    pub trigger: DeoptimizationTrigger,
    pub candidate_action_digest: Sha256Digest,
    pub candidate_state_digest: Sha256Digest,
    pub candidate_closure_manifest_digest: Sha256Digest,
    pub prior_work_receipt_digest: Sha256Digest,
    pub kernel_binding_digest: Sha256Digest,
    pub kernel_admission_digest: Sha256Digest,
    pub plan_digest: Sha256Digest,
}

impl DeoptimizationPlanRecord {
    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        self.safepoint.validate()?;
        self.claim().validate()?;
        let expected = digest_value(
            PLAN_DOMAIN,
            &json!({
                "candidate_action_digest": self.candidate_action_digest,
                "candidate_closure_manifest_digest": self.candidate_closure_manifest_digest,
                "candidate_state_digest": self.candidate_state_digest,
                "contract_version": self.contract_version,
                "kernel_admission_digest": self.kernel_admission_digest,
                "kernel_binding_digest": self.kernel_binding_digest,
                "prior_work_receipt_digest": self.prior_work_receipt_digest,
                "safepoint_certificate_digest": self.safepoint.certificate_digest,
                "trigger": self.trigger,
            }),
        );
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION || expected != self.plan_digest
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::PlanDigestMismatch,
                "deoptimization plan record does not replay",
            ));
        }
        Ok(())
    }

    pub fn claim(&self) -> DeoptimizationPlanClaim {
        DeoptimizationPlanClaim {
            schema_version: DEOPTIMIZATION_PLAN_SCHEMA_VERSION.into(),
            safepoint_certificate_digest: self.safepoint.certificate_digest,
            trigger: self.trigger.clone(),
            candidate_action_digest: self.candidate_action_digest,
            candidate_state_digest: self.candidate_state_digest,
            candidate_closure_manifest_digest: self.candidate_closure_manifest_digest,
            prior_work_receipt_digest: self.prior_work_receipt_digest,
            kernel_binding_digest: self.kernel_binding_digest,
            kernel_admission_digest: self.kernel_admission_digest,
            plan_digest: self.plan_digest,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineRestorationClaim {
    schema_version: String,
    plan_digest: Sha256Digest,
    safepoint_certificate_digest: Sha256Digest,
    transaction_receipt_digest: Sha256Digest,
    restored_project_root: Sha256Digest,
    restored_external_inventory_digest: Sha256Digest,
    restored_reasoning_contract_digest: Sha256Digest,
    restored_fixed_model_digest: Sha256Digest,
    restored_reasoning_entry_digest: Sha256Digest,
    raw_baseline_identity_digest: Sha256Digest,
    raw_baseline_input_digest: Sha256Digest,
    raw_decision_view_digest: Sha256Digest,
    candidate_overlay_disposition_digest: Sha256Digest,
    visible_buffer_disposition_digest: Sha256Digest,
    prior_receipt_head_digest: Sha256Digest,
    successor_receipt_head_digest: Sha256Digest,
    restoration_verifier_identity_digest: Sha256Digest,
    deoptimization_usage: RouteUsage,
}

impl BaselineRestorationClaim {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plan: &DeoptimizationPlan,
        transaction_receipt_digest: Sha256Digest,
        candidate_overlay_disposition_digest: Sha256Digest,
        visible_buffer_disposition_digest: Sha256Digest,
        successor_receipt_head_digest: Sha256Digest,
        restoration_verifier_identity_digest: Sha256Digest,
        deoptimization_usage: RouteUsage,
    ) -> Result<Self, DeoptimizationError> {
        let safepoint = plan.safepoint.claim();
        let claim = Self {
            schema_version: BASELINE_RESTORATION_SCHEMA_VERSION.into(),
            plan_digest: plan.plan_digest,
            safepoint_certificate_digest: plan.safepoint.certificate_digest,
            transaction_receipt_digest,
            restored_project_root: safepoint.project_snapshot_root,
            restored_external_inventory_digest: safepoint.external_state_inventory_digest,
            restored_reasoning_contract_digest: safepoint.baseline_reasoning_contract_digest,
            restored_fixed_model_digest: safepoint.baseline_reasoning_contract.model_identity(),
            restored_reasoning_entry_digest: safepoint.reasoning_entry.digest()?,
            raw_baseline_identity_digest: safepoint.raw_baseline_identity_digest,
            raw_baseline_input_digest: safepoint.raw_baseline_input_digest,
            raw_decision_view_digest: safepoint.raw_decision_view_digest,
            candidate_overlay_disposition_digest,
            visible_buffer_disposition_digest,
            prior_receipt_head_digest: safepoint.capture_receipt_head_digest,
            successor_receipt_head_digest,
            restoration_verifier_identity_digest,
            deoptimization_usage,
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        if self.schema_version != BASELINE_RESTORATION_SCHEMA_VERSION {
            return Err(deopt_error(
                DeoptimizationFailureCode::SchemaVersionMismatch,
                "baseline restoration schema is unsupported",
            ));
        }
        require_nonzero(
            "baseline restoration claim",
            &[
                self.plan_digest,
                self.safepoint_certificate_digest,
                self.transaction_receipt_digest,
                self.restored_project_root,
                self.restored_external_inventory_digest,
                self.restored_reasoning_contract_digest,
                self.restored_fixed_model_digest,
                self.restored_reasoning_entry_digest,
                self.raw_baseline_identity_digest,
                self.raw_baseline_input_digest,
                self.raw_decision_view_digest,
                self.candidate_overlay_disposition_digest,
                self.visible_buffer_disposition_digest,
                self.prior_receipt_head_digest,
                self.successor_receipt_head_digest,
                self.restoration_verifier_identity_digest,
            ],
        )?;
        if self.successor_receipt_head_digest == self.prior_receipt_head_digest {
            return Err(deopt_error(
                DeoptimizationFailureCode::ReceiptChainMismatch,
                "restoration must advance the receipt head",
            ));
        }
        self.deoptimization_usage.validate()
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DeoptimizationError> {
        let claim: Self = decode_canonical(bytes)?;
        claim.validate()?;
        Ok(claim)
    }

    pub fn digest(&self) -> Result<Sha256Digest, DeoptimizationError> {
        Ok(domain_digest(
            RESTORATION_CLAIM_DOMAIN,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestoredTransactionRecord {
    pub receipt_digest: Sha256Digest,
    pub action_digest: Sha256Digest,
    pub closure_manifest_digest: Sha256Digest,
    pub baseline_state: Sha256Digest,
    pub candidate_state: Sha256Digest,
    pub external_inventory_digest: Sha256Digest,
    pub resource_count: u16,
    pub external_resource_count: u16,
    pub recovery_outcome: RecoveryOutcome,
}

/// Opaque restoration authority. It can mint one frozen baseline invocation,
/// but it cannot enter G8 or claim baseline execution or publication.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineResumePermit {
    contract_version: u16,
    plan: DeoptimizationPlan,
    restoration_claim: BaselineRestorationClaim,
    restored_transaction: RestoredTransactionRecord,
    restoration_claim_digest: Sha256Digest,
    evidence_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
    permit_digest: Sha256Digest,
}

impl BaselineResumePermit {
    pub fn verify_restoration(
        plan: DeoptimizationPlan,
        transaction: TransactionReceipt,
        restoration_claim: BaselineRestorationClaim,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, DeoptimizationError> {
        plan.validate()?;
        restoration_claim.validate()?;
        transaction.canonical_bytes().map_err(|error| {
            deopt_error(
                DeoptimizationFailureCode::TransactionMismatch,
                format!("transaction receipt is invalid: {error}"),
            )
        })?;
        let safepoint = plan.safepoint.claim();
        if transaction.disposition() != TransactionDisposition::BaselineRootRecovered
            || transaction.restoration_scope() != RestorationScope::DeclaredEffectClosure
            || transaction.external_restoration_debt_count() != 0
            || transaction.baseline_state() != safepoint.project_snapshot_root
            || transaction.observed_root() != safepoint.project_snapshot_root
            || transaction.external_inventory_digest() != safepoint.external_state_inventory_digest
            || transaction.action_digest() != plan.candidate_action_digest
            || transaction.candidate_state() != plan.candidate_state_digest
            || transaction.closure_manifest_digest() != plan.candidate_closure_manifest_digest
            || transaction.receipt_digest() != restoration_claim.transaction_receipt_digest
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::TransactionMismatch,
                "transaction did not restore the complete frozen effect closure",
            ));
        }
        if restoration_claim.plan_digest != plan.plan_digest
            || restoration_claim.safepoint_certificate_digest != plan.safepoint.certificate_digest
            || restoration_claim.restored_project_root != safepoint.project_snapshot_root
            || restoration_claim.restored_external_inventory_digest
                != safepoint.external_state_inventory_digest
            || restoration_claim.restored_reasoning_contract_digest
                != safepoint.baseline_reasoning_contract_digest
            || restoration_claim.restored_fixed_model_digest
                != safepoint.baseline_reasoning_contract.model_identity()
            || restoration_claim.restored_reasoning_entry_digest
                != safepoint.reasoning_entry.digest()?
            || restoration_claim.raw_baseline_identity_digest
                != safepoint.raw_baseline_identity_digest
            || restoration_claim.raw_baseline_input_digest != safepoint.raw_baseline_input_digest
            || restoration_claim.raw_decision_view_digest != safepoint.raw_decision_view_digest
            || restoration_claim.prior_receipt_head_digest != safepoint.capture_receipt_head_digest
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::RestorationMismatch,
                "restoration proof differs from the frozen safepoint",
            ));
        }
        if !restoration_claim.deoptimization_usage.within(
            &safepoint.reserve.deoptimization_envelope,
            &safepoint.reserve.deoptimization_work_limit,
        ) {
            return Err(deopt_error(
                DeoptimizationFailureCode::ResourceReserveExceeded,
                "deoptimization work exceeds the frozen fallback reserve",
            ));
        }
        verify_exact_successful_payload(&restoration_claim.canonical_bytes()?, evidence)?;
        let verifier_identity_digest = deoptimization_verifier_identity(evidence);
        if verifier_identity_digest != restoration_claim.restoration_verifier_identity_digest {
            return Err(deopt_error(
                DeoptimizationFailureCode::VerifierIdentityMismatch,
                "restoration verifier differs from the claim route",
            ));
        }
        let restored_transaction = RestoredTransactionRecord {
            receipt_digest: transaction.receipt_digest(),
            action_digest: transaction.action_digest(),
            closure_manifest_digest: transaction.closure_manifest_digest(),
            baseline_state: transaction.baseline_state(),
            candidate_state: transaction.candidate_state(),
            external_inventory_digest: transaction.external_inventory_digest(),
            resource_count: transaction.resource_count(),
            external_resource_count: transaction.external_resource_count(),
            recovery_outcome: transaction.recovery_outcome(),
        };
        let restoration_claim_digest = restoration_claim.digest()?;
        let evidence_digest = verified_evidence_digest(evidence)?;
        let permit_digest = resume_permit_digest(
            plan.plan_digest,
            restoration_claim_digest,
            transaction.receipt_digest(),
            evidence_digest,
            verifier_identity_digest,
        );
        let permit = Self {
            contract_version: DEOPTIMIZATION_CONTRACT_VERSION,
            plan,
            restoration_claim,
            restored_transaction,
            restoration_claim_digest,
            evidence_digest,
            verifier_identity_digest,
            permit_digest,
        };
        permit.validate()?;
        Ok(permit)
    }

    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION {
            return Err(deopt_error(
                DeoptimizationFailureCode::SchemaVersionMismatch,
                "baseline resume permit contract is unsupported",
            ));
        }
        self.plan.validate()?;
        self.restoration_claim.validate()?;
        require_nonzero(
            "baseline resume proof",
            &[
                self.restoration_claim_digest,
                self.evidence_digest,
                self.verifier_identity_digest,
                self.permit_digest,
            ],
        )?;
        let safepoint = self.plan.safepoint.claim();
        if self.restoration_claim.digest()? != self.restoration_claim_digest
            || self.restored_transaction.receipt_digest
                != self.restoration_claim.transaction_receipt_digest
            || self.restored_transaction.action_digest != self.plan.candidate_action_digest
            || self.restored_transaction.closure_manifest_digest
                != self.plan.candidate_closure_manifest_digest
            || self.restored_transaction.baseline_state != safepoint.project_snapshot_root
            || self.restored_transaction.candidate_state != self.plan.candidate_state_digest
            || self.restored_transaction.external_inventory_digest
                != safepoint.external_state_inventory_digest
            || self.verifier_identity_digest
                != self.restoration_claim.restoration_verifier_identity_digest
            || resume_permit_digest(
                self.plan.plan_digest,
                self.restoration_claim_digest,
                self.restored_transaction.receipt_digest,
                self.evidence_digest,
                self.verifier_identity_digest,
            ) != self.permit_digest
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::CertificateDigestMismatch,
                "baseline resume permit does not replay against its route evidence",
            ));
        }
        Ok(())
    }

    pub fn into_invocation(self) -> Result<FrozenBaselineInvocation, DeoptimizationError> {
        self.validate()?;
        let resume_record = self.record();
        let safepoint = self.plan.safepoint.claim();
        let mut invocation = FrozenBaselineInvocation {
            resume_record,
            contract_version: DEOPTIMIZATION_CONTRACT_VERSION,
            resume_permit_digest: self.permit_digest,
            predecessor_receipt_head_digest: self.restoration_claim.successor_receipt_head_digest,
            transaction_receipt_digest: self.restored_transaction.receipt_digest,
            project_snapshot_root: safepoint.project_snapshot_root,
            raw_baseline_identity_digest: safepoint.raw_baseline_identity_digest,
            raw_baseline_input_digest: safepoint.raw_baseline_input_digest,
            raw_decision_view_digest: safepoint.raw_decision_view_digest,
            comparison_identity_digest: safepoint.comparison_identity_digest,
            assembly_contract_digest: safepoint.assembly_contract_digest,
            baseline_engine_contract_digest: safepoint.baseline_engine_contract_digest,
            effect_schema_digest: safepoint.effect_schema_digest,
            baseline_reasoning_contract: safepoint.baseline_reasoning_contract.clone(),
            baseline_reasoning_contract_digest: safepoint.baseline_reasoning_contract_digest,
            reasoning_entry: safepoint.reasoning_entry.clone(),
            sampler_randomness_identity_digest: safepoint.sampler_randomness_identity_digest,
            baseline_verifier_identity_digest: safepoint.baseline_verifier_identity_digest,
            raw_baseline_envelope: safepoint.reserve.raw_baseline_envelope,
            raw_baseline_work_limit: safepoint.reserve.raw_baseline_work_limit,
            invocation_digest: Sha256Digest::ZERO,
        };
        invocation.invocation_digest = invocation.expected_digest()?;
        invocation.validate()?;
        Ok(invocation)
    }

    pub fn record(&self) -> BaselineResumeReceiptRecord {
        BaselineResumeReceiptRecord {
            contract_version: self.contract_version,
            plan: self.plan.record(),
            restoration_claim: self.restoration_claim.clone(),
            restored_transaction: self.restored_transaction.clone(),
            restoration_claim_digest: self.restoration_claim_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            permit_digest: self.permit_digest,
        }
    }

    pub const fn permit_digest(&self) -> Sha256Digest {
        self.permit_digest
    }
    pub const fn restored_transaction(&self) -> &RestoredTransactionRecord {
        &self.restored_transaction
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineResumeReceiptRecord {
    pub contract_version: u16,
    pub plan: DeoptimizationPlanRecord,
    pub restoration_claim: BaselineRestorationClaim,
    pub restored_transaction: RestoredTransactionRecord,
    pub restoration_claim_digest: Sha256Digest,
    pub evidence_digest: Sha256Digest,
    pub verifier_identity_digest: Sha256Digest,
    pub permit_digest: Sha256Digest,
}

impl BaselineResumeReceiptRecord {
    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        self.plan.validate()?;
        self.restoration_claim.validate()?;
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION
            || self.restoration_claim.plan_digest != self.plan.plan_digest
            || self.restoration_claim.safepoint_certificate_digest
                != self.plan.safepoint.certificate_digest
            || self.restoration_claim.digest()? != self.restoration_claim_digest
            || self.restored_transaction.receipt_digest
                != self.restoration_claim.transaction_receipt_digest
            || self.restored_transaction.action_digest != self.plan.candidate_action_digest
            || self.restored_transaction.closure_manifest_digest
                != self.plan.candidate_closure_manifest_digest
            || self.restored_transaction.baseline_state
                != self.plan.safepoint.claim.project_snapshot_root
            || self.restored_transaction.candidate_state != self.plan.candidate_state_digest
            || self.restored_transaction.external_inventory_digest
                != self.plan.safepoint.claim.external_state_inventory_digest
            || self.verifier_identity_digest
                != self.restoration_claim.restoration_verifier_identity_digest
            || resume_permit_digest(
                self.plan.plan_digest,
                self.restoration_claim_digest,
                self.restored_transaction.receipt_digest,
                self.evidence_digest,
                self.verifier_identity_digest,
            ) != self.permit_digest
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::CertificateDigestMismatch,
                "baseline resume record does not replay",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DeoptimizationError> {
        let record: Self = decode_canonical(bytes)?;
        record.validate()?;
        Ok(record)
    }
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenBaselineInvocation {
    resume_record: BaselineResumeReceiptRecord,
    contract_version: u16,
    resume_permit_digest: Sha256Digest,
    predecessor_receipt_head_digest: Sha256Digest,
    transaction_receipt_digest: Sha256Digest,
    project_snapshot_root: Sha256Digest,
    raw_baseline_identity_digest: Sha256Digest,
    raw_baseline_input_digest: Sha256Digest,
    raw_decision_view_digest: Sha256Digest,
    comparison_identity_digest: Sha256Digest,
    assembly_contract_digest: Sha256Digest,
    baseline_engine_contract_digest: Sha256Digest,
    effect_schema_digest: Sha256Digest,
    baseline_reasoning_contract: ReasoningContract,
    baseline_reasoning_contract_digest: Sha256Digest,
    reasoning_entry: BaselineReasoningEntry,
    sampler_randomness_identity_digest: Sha256Digest,
    baseline_verifier_identity_digest: Sha256Digest,
    raw_baseline_envelope: WorkerEnvelope,
    raw_baseline_work_limit: RaccWork,
    invocation_digest: Sha256Digest,
}

impl FrozenBaselineInvocation {
    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION {
            return Err(deopt_error(
                DeoptimizationFailureCode::SchemaVersionMismatch,
                "frozen baseline invocation contract is unsupported",
            ));
        }
        self.resume_record.validate()?;
        if self.resume_record.permit_digest != self.resume_permit_digest
            || self.resume_record.restored_transaction.receipt_digest
                != self.transaction_receipt_digest
            || self
                .resume_record
                .restoration_claim
                .successor_receipt_head_digest
                != self.predecessor_receipt_head_digest
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::SafepointBindingMismatch,
                "baseline invocation differs from the verified resume route",
            ));
        }
        require_nonzero(
            "frozen baseline invocation",
            &[
                self.resume_permit_digest,
                self.predecessor_receipt_head_digest,
                self.transaction_receipt_digest,
                self.project_snapshot_root,
                self.raw_baseline_identity_digest,
                self.raw_baseline_input_digest,
                self.raw_decision_view_digest,
                self.comparison_identity_digest,
                self.assembly_contract_digest,
                self.baseline_engine_contract_digest,
                self.effect_schema_digest,
                self.baseline_reasoning_contract_digest,
                self.sampler_randomness_identity_digest,
                self.baseline_verifier_identity_digest,
            ],
        )?;
        self.baseline_reasoning_contract
            .validate()
            .map_err(|error| reasoning_error(error.to_string()))?;
        validate_envelope("raw baseline", &self.raw_baseline_envelope)?;
        self.raw_baseline_work_limit
            .validate_limit("raw baseline work")?;
        if self
            .baseline_reasoning_contract
            .identity_digest()
            .map_err(|error| reasoning_error(error.to_string()))?
            != self.baseline_reasoning_contract_digest
            || self.expected_digest()? != self.invocation_digest
            || baseline_invocation_digest_from_resume_record(&self.resume_record)?
                != self.invocation_digest
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::CertificateDigestMismatch,
                "frozen baseline invocation does not bind its exact contract",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<Sha256Digest, DeoptimizationError> {
        Ok(digest_value(
            INVOCATION_DOMAIN,
            &json!({
                "assembly_contract_digest": self.assembly_contract_digest,
                "baseline_reasoning_contract": self.baseline_reasoning_contract,
                "baseline_reasoning_contract_digest": self.baseline_reasoning_contract_digest,
                "baseline_verifier_identity_digest": self.baseline_verifier_identity_digest,
                "comparison_identity_digest": self.comparison_identity_digest,
                "contract_version": self.contract_version,
                "effect_schema_digest": self.effect_schema_digest,
                "predecessor_receipt_head_digest": self.predecessor_receipt_head_digest,
                "project_snapshot_root": self.project_snapshot_root,
                "raw_baseline_envelope": self.raw_baseline_envelope,
                "raw_baseline_identity_digest": self.raw_baseline_identity_digest,
                "raw_baseline_input_digest": self.raw_baseline_input_digest,
                "raw_baseline_work_limit": self.raw_baseline_work_limit,
                "raw_decision_view_digest": self.raw_decision_view_digest,
                "baseline_engine_contract_digest": self.baseline_engine_contract_digest,
                "reasoning_entry": self.reasoning_entry,
                "resume_permit_digest": self.resume_permit_digest,
                "sampler_randomness_identity_digest": self.sampler_randomness_identity_digest,
                "transaction_receipt_digest": self.transaction_receipt_digest,
            }),
        ))
    }

    pub const fn invocation_digest(&self) -> Sha256Digest {
        self.invocation_digest
    }
    pub const fn raw_baseline_identity_digest(&self) -> Sha256Digest {
        self.raw_baseline_identity_digest
    }
    pub const fn project_snapshot_root(&self) -> Sha256Digest {
        self.project_snapshot_root
    }
}

fn baseline_invocation_digest_from_resume_record(
    record: &BaselineResumeReceiptRecord,
) -> Result<Sha256Digest, DeoptimizationError> {
    record.validate()?;
    let safepoint = &record.plan.safepoint.claim;
    Ok(digest_value(
        INVOCATION_DOMAIN,
        &json!({
            "assembly_contract_digest": safepoint.assembly_contract_digest,
            "baseline_reasoning_contract": safepoint.baseline_reasoning_contract,
            "baseline_reasoning_contract_digest": safepoint.baseline_reasoning_contract_digest,
            "baseline_verifier_identity_digest": safepoint.baseline_verifier_identity_digest,
            "comparison_identity_digest": safepoint.comparison_identity_digest,
            "contract_version": DEOPTIMIZATION_CONTRACT_VERSION,
            "effect_schema_digest": safepoint.effect_schema_digest,
            "predecessor_receipt_head_digest": record.restoration_claim.successor_receipt_head_digest,
            "project_snapshot_root": safepoint.project_snapshot_root,
            "raw_baseline_envelope": safepoint.reserve.raw_baseline_envelope,
            "raw_baseline_identity_digest": safepoint.raw_baseline_identity_digest,
            "raw_baseline_input_digest": safepoint.raw_baseline_input_digest,
            "raw_baseline_work_limit": safepoint.reserve.raw_baseline_work_limit,
            "raw_decision_view_digest": safepoint.raw_decision_view_digest,
            "baseline_engine_contract_digest": safepoint.baseline_engine_contract_digest,
            "reasoning_entry": safepoint.reasoning_entry,
            "resume_permit_digest": record.permit_digest,
            "sampler_randomness_identity_digest": safepoint.sampler_randomness_identity_digest,
            "transaction_receipt_digest": record.restored_transaction.receipt_digest,
        }),
    ))
}

fn resume_permit_digest(
    plan_digest: Sha256Digest,
    restoration_claim_digest: Sha256Digest,
    transaction_receipt_digest: Sha256Digest,
    evidence_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
) -> Sha256Digest {
    digest_value(
        RESUME_PERMIT_DOMAIN,
        &json!({
            "contract_version": DEOPTIMIZATION_CONTRACT_VERSION,
            "evidence_digest": evidence_digest,
            "plan_digest": plan_digest,
            "restoration_claim_digest": restoration_claim_digest,
            "transaction_receipt_digest": transaction_receipt_digest,
            "verifier_identity_digest": verifier_identity_digest,
        }),
    )
}

pub fn deoptimization_verifier_identity(evidence: &VerifiedEvidence<'_, '_>) -> Sha256Digest {
    let provenance = evidence.provenance();
    digest_value(
        VERIFIER_DOMAIN,
        &json!({
            "index_id": provenance.index_id,
            "index_version": provenance.index_version,
            "operator_id": provenance.operator_id,
            "operator_version": provenance.operator_version,
            "parser_id": provenance.parser_id,
            "parser_version": provenance.parser_version,
        }),
    )
}

fn verify_exact_successful_payload(
    expected: &[u8],
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<(), DeoptimizationError> {
    match (evidence.query(), &evidence.certificate().completeness) {
        (Query::BuildReceipt { .. }, CompletenessWitness::BuildReceipt { exit_code: 0, .. })
        | (Query::TestTrace { .. }, CompletenessWitness::TestTrace { exit_code: 0, .. }) => {}
        _ => {
            return Err(deopt_error(
                DeoptimizationFailureCode::UnsupportedEvidenceClass,
                "deoptimization requires a successful verified build or test trace",
            ));
        }
    }
    if evidence.payload() != expected {
        return Err(deopt_error(
            DeoptimizationFailureCode::EvidencePayloadMismatch,
            "verified evidence payload differs from the exact canonical claim",
        ));
    }
    Ok(())
}

fn verified_evidence_digest(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<Sha256Digest, DeoptimizationError> {
    Ok(Sha256Digest::from_bytes(
        evidence
            .certificate()
            .canonical_digest()
            .map_err(|error| json_error(error.to_string()))?,
    ))
}

pub fn deoptimization_contract_manifest() -> Value {
    json!({
        "baseline_entry_modes": ["exact_native_continuation", "canonical_clean_start"],
        "baseline_invocation_authority": "linear_resume_permit_to_invocation_then_verified_execution_receipt",
        "canonical_encoding": "sorted_key_json_no_whitespace",
        "contract_version": DEOPTIMIZATION_CONTRACT_VERSION,
        "deoptimization_triggers": ["recovery_unknown", "quality_baseline_selection", "fail_closed"],
        "linked_contracts": {
            "dominance_complete_recovery": dcr_contract_digest(),
            "quality_envelope": quality_envelope_contract_digest(),
            "reasoning_contract": reasoning_contract_digest(),
            "transaction": transaction_contract_digest(),
        },
        "exact_restoration_requirements": [
            "validated_transaction_receipt",
            "baseline_root_recovered",
            "declared_effect_closure",
            "zero_external_restoration_debt",
            "frozen_reasoning_entry",
            "frozen_raw_baseline_identity",
            "verified_canonical_restoration_claim",
            "bounded_and_charged_fallback_work",
            "advanced_receipt_head",
            "verified_exact_raw_baseline_execution",
            "baseline_transaction_successor_output_and_effect_bindings",
            "originating_kernel_binding_and_admission",
        ],
        "forbidden_promotions": [
            "journal_root_only_to_exact_external_restoration",
            "clean_restart_to_native_continuation",
            "unverified_boolean_to_restoration_authority",
            "resume_permit_to_baseline_execution_or_publication_claim",
            "resume_permit_directly_to_g8_fallback_closure",
        ],
        "max_canonical_bytes": DEOPTIMIZATION_MAX_CANONICAL_BYTES,
        "proof_carrier": "zero_cert::VerifiedEvidence_successful_build_or_test_exact_claim_payload",
        "resource_arithmetic": "integer_native_coordinates_no_scalar_laundering",
    })
}

pub fn deoptimization_contract_digest() -> Sha256Digest {
    digest_value(CONTRACT_DOMAIN, &deoptimization_contract_manifest())
}

fn canonical_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, DeoptimizationError> {
    let value = serde_json::to_value(value).map_err(|error| json_error(error.to_string()))?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() > DEOPTIMIZATION_MAX_CANONICAL_BYTES {
        return Err(deopt_error(
            DeoptimizationFailureCode::CanonicalPayloadTooLarge,
            "deoptimization payload exceeds its canonical byte bound",
        ));
    }
    Ok(bytes)
}

fn decode_canonical<T>(bytes: &[u8]) -> Result<T, DeoptimizationError>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    if bytes.len() > DEOPTIMIZATION_MAX_CANONICAL_BYTES {
        return Err(deopt_error(
            DeoptimizationFailureCode::CanonicalPayloadTooLarge,
            "deoptimization payload exceeds its canonical byte bound",
        ));
    }
    let value = serde_json::from_slice(bytes).map_err(|error| json_error(error.to_string()))?;
    if canonical_bytes(&value)? != bytes {
        return Err(deopt_error(
            DeoptimizationFailureCode::NonCanonicalEncoding,
            "deoptimization bytes are not canonical sorted-key JSON",
        ));
    }
    Ok(value)
}

fn digest_value(domain: &[u8], value: &Value) -> Sha256Digest {
    let canonical = canonical_json(value);
    let mut bytes = Vec::with_capacity(domain.len() + canonical.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(canonical.as_bytes());
    Sha256Digest::from_bytes(sha256(&bytes))
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> Sha256Digest {
    let mut value = Vec::with_capacity(domain.len() + bytes.len());
    value.extend_from_slice(domain);
    value.extend_from_slice(bytes);
    Sha256Digest::from_bytes(sha256(&value))
}

fn require_nonzero(
    label: &'static str,
    values: &[Sha256Digest],
) -> Result<(), DeoptimizationError> {
    if values.contains(&Sha256Digest::ZERO) {
        Err(deopt_error(
            DeoptimizationFailureCode::ZeroDigest,
            format!("{label} contains a zero digest"),
        ))
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineExecutionClaim {
    schema_version: String,
    invocation_digest: Sha256Digest,
    resume_permit_digest: Sha256Digest,
    transaction_receipt_digest: Sha256Digest,
    project_snapshot_root: Sha256Digest,
    raw_baseline_identity_digest: Sha256Digest,
    raw_baseline_input_digest: Sha256Digest,
    raw_decision_view_digest: Sha256Digest,
    baseline_reasoning_contract_digest: Sha256Digest,
    reasoning_entry_digest: Sha256Digest,
    predecessor_receipt_head_digest: Sha256Digest,
    output_digest: Sha256Digest,
    effects_digest: Sha256Digest,
    baseline_action_digest: Sha256Digest,
    baseline_acceptance_digest: Sha256Digest,
    baseline_successor_root: Sha256Digest,
    baseline_transaction_receipt_digest: Sha256Digest,
    raw_baseline_usage: RouteUsage,
    successor_receipt_head_digest: Sha256Digest,
    execution_verifier_identity_digest: Sha256Digest,
}

impl BaselineExecutionClaim {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        invocation: &FrozenBaselineInvocation,
        output_digest: Sha256Digest,
        effects_digest: Sha256Digest,
        baseline_action_digest: Sha256Digest,
        baseline_acceptance_digest: Sha256Digest,
        baseline_successor_root: Sha256Digest,
        baseline_transaction_receipt_digest: Sha256Digest,
        raw_baseline_usage: RouteUsage,
        successor_receipt_head_digest: Sha256Digest,
        execution_verifier_identity_digest: Sha256Digest,
    ) -> Result<Self, DeoptimizationError> {
        invocation.validate()?;
        let claim = Self {
            schema_version: BASELINE_EXECUTION_SCHEMA_VERSION.into(),
            invocation_digest: invocation.invocation_digest,
            resume_permit_digest: invocation.resume_permit_digest,
            transaction_receipt_digest: invocation.transaction_receipt_digest,
            project_snapshot_root: invocation.project_snapshot_root,
            raw_baseline_identity_digest: invocation.raw_baseline_identity_digest,
            raw_baseline_input_digest: invocation.raw_baseline_input_digest,
            raw_decision_view_digest: invocation.raw_decision_view_digest,
            baseline_reasoning_contract_digest: invocation.baseline_reasoning_contract_digest,
            reasoning_entry_digest: invocation.reasoning_entry.digest()?,
            predecessor_receipt_head_digest: invocation.predecessor_receipt_head_digest,
            output_digest,
            effects_digest,
            baseline_action_digest,
            baseline_acceptance_digest,
            baseline_successor_root,
            baseline_transaction_receipt_digest,
            raw_baseline_usage,
            successor_receipt_head_digest,
            execution_verifier_identity_digest,
        };
        claim.validate()?;
        if !claim.raw_baseline_usage.within(
            &invocation.raw_baseline_envelope,
            &invocation.raw_baseline_work_limit,
        ) {
            return Err(deopt_error(
                DeoptimizationFailureCode::ResourceReserveExceeded,
                "raw baseline execution exceeds its frozen reserve",
            ));
        }
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        if self.schema_version != BASELINE_EXECUTION_SCHEMA_VERSION {
            return Err(deopt_error(
                DeoptimizationFailureCode::SchemaVersionMismatch,
                "baseline execution claim schema is unsupported",
            ));
        }
        require_nonzero(
            "baseline execution claim",
            &[
                self.invocation_digest,
                self.resume_permit_digest,
                self.transaction_receipt_digest,
                self.project_snapshot_root,
                self.raw_baseline_identity_digest,
                self.raw_baseline_input_digest,
                self.raw_decision_view_digest,
                self.baseline_reasoning_contract_digest,
                self.reasoning_entry_digest,
                self.predecessor_receipt_head_digest,
                self.output_digest,
                self.effects_digest,
                self.baseline_action_digest,
                self.baseline_acceptance_digest,
                self.baseline_successor_root,
                self.baseline_transaction_receipt_digest,
                self.successor_receipt_head_digest,
                self.execution_verifier_identity_digest,
            ],
        )?;
        self.raw_baseline_usage.validate()?;
        if self.successor_receipt_head_digest == self.predecessor_receipt_head_digest {
            return Err(deopt_error(
                DeoptimizationFailureCode::ReceiptChainMismatch,
                "baseline execution did not advance the receipt chain",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DeoptimizationError> {
        let claim: Self = decode_canonical(bytes)?;
        claim.validate()?;
        Ok(claim)
    }

    pub fn digest(&self) -> Result<Sha256Digest, DeoptimizationError> {
        Ok(domain_digest(
            EXECUTION_CLAIM_DOMAIN,
            &self.canonical_bytes()?,
        ))
    }
}

/// Opaque proof that the exact frozen raw baseline completed successfully.
/// It authorizes G8 fallback closure, not publication or native durability alone.
#[derive(Debug)]
pub struct BaselineExecutionReceipt {
    resume_record: BaselineResumeReceiptRecord,
    invocation_digest: Sha256Digest,
    execution_claim: BaselineExecutionClaim,
    execution_claim_digest: Sha256Digest,
    evidence_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
    receipt_digest: Sha256Digest,
}

impl BaselineExecutionReceipt {
    pub fn verify_execution(
        invocation: FrozenBaselineInvocation,
        execution_claim: BaselineExecutionClaim,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, DeoptimizationError> {
        invocation.validate()?;
        execution_claim.validate()?;
        if execution_claim.invocation_digest != invocation.invocation_digest
            || execution_claim.resume_permit_digest != invocation.resume_permit_digest
            || execution_claim.transaction_receipt_digest != invocation.transaction_receipt_digest
            || execution_claim.project_snapshot_root != invocation.project_snapshot_root
            || execution_claim.raw_baseline_identity_digest
                != invocation.raw_baseline_identity_digest
            || execution_claim.raw_baseline_input_digest != invocation.raw_baseline_input_digest
            || execution_claim.raw_decision_view_digest != invocation.raw_decision_view_digest
            || execution_claim.baseline_reasoning_contract_digest
                != invocation.baseline_reasoning_contract_digest
            || execution_claim.reasoning_entry_digest != invocation.reasoning_entry.digest()?
            || execution_claim.predecessor_receipt_head_digest
                != invocation.predecessor_receipt_head_digest
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::BaselineExecutionMismatch,
                "baseline execution differs from the frozen invocation",
            ));
        }
        if !execution_claim.raw_baseline_usage.within(
            &invocation.raw_baseline_envelope,
            &invocation.raw_baseline_work_limit,
        ) {
            return Err(deopt_error(
                DeoptimizationFailureCode::ResourceReserveExceeded,
                "raw baseline execution exceeds its frozen reserve",
            ));
        }
        verify_exact_successful_payload(&execution_claim.canonical_bytes()?, evidence)?;
        let verifier_identity_digest = deoptimization_verifier_identity(evidence);
        if verifier_identity_digest != execution_claim.execution_verifier_identity_digest {
            return Err(deopt_error(
                DeoptimizationFailureCode::VerifierIdentityMismatch,
                "baseline execution verifier differs from the claim route",
            ));
        }
        let execution_claim_digest = execution_claim.digest()?;
        let evidence_digest = verified_evidence_digest(evidence)?;
        let resume_record = invocation.resume_record;
        let receipt_digest = baseline_execution_receipt_digest(
            resume_record.permit_digest,
            invocation.invocation_digest,
            execution_claim_digest,
            evidence_digest,
            verifier_identity_digest,
        );
        let receipt = Self {
            resume_record,
            invocation_digest: invocation.invocation_digest,
            execution_claim,
            execution_claim_digest,
            evidence_digest,
            verifier_identity_digest,
            receipt_digest,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        self.record().validate()
    }

    pub fn record(&self) -> BaselineExecutionReceiptRecord {
        BaselineExecutionReceiptRecord {
            contract_version: DEOPTIMIZATION_CONTRACT_VERSION,
            resume_record: self.resume_record.clone(),
            invocation_digest: self.invocation_digest,
            execution_claim: self.execution_claim.clone(),
            execution_claim_digest: self.execution_claim_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            receipt_digest: self.receipt_digest,
        }
    }

    pub const fn receipt_digest(&self) -> Sha256Digest {
        self.receipt_digest
    }

    pub const fn restored_transaction(&self) -> &RestoredTransactionRecord {
        &self.resume_record.restored_transaction
    }

    pub const fn project_snapshot_root(&self) -> Sha256Digest {
        self.resume_record
            .plan
            .safepoint
            .claim
            .project_snapshot_root
    }

    pub const fn baseline_successor_root(&self) -> Sha256Digest {
        self.execution_claim.baseline_successor_root
    }

    pub const fn baseline_transaction_receipt_digest(&self) -> Sha256Digest {
        self.execution_claim.baseline_transaction_receipt_digest
    }

    pub const fn baseline_action_digest(&self) -> Sha256Digest {
        self.execution_claim.baseline_action_digest
    }

    pub const fn baseline_acceptance_digest(&self) -> Sha256Digest {
        self.execution_claim.baseline_acceptance_digest
    }

    pub const fn kernel_binding_digest(&self) -> Sha256Digest {
        self.resume_record.plan.kernel_binding_digest
    }

    pub const fn kernel_admission_digest(&self) -> Sha256Digest {
        self.resume_record.plan.kernel_admission_digest
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineExecutionReceiptRecord {
    pub contract_version: u16,
    pub resume_record: BaselineResumeReceiptRecord,
    pub invocation_digest: Sha256Digest,
    pub execution_claim: BaselineExecutionClaim,
    pub execution_claim_digest: Sha256Digest,
    pub evidence_digest: Sha256Digest,
    pub verifier_identity_digest: Sha256Digest,
    pub receipt_digest: Sha256Digest,
}

impl BaselineExecutionReceiptRecord {
    pub fn validate(&self) -> Result<(), DeoptimizationError> {
        self.resume_record.validate()?;
        self.execution_claim.validate()?;
        let safepoint = &self.resume_record.plan.safepoint.claim;
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION
            || baseline_invocation_digest_from_resume_record(&self.resume_record)?
                != self.invocation_digest
            || self.execution_claim.resume_permit_digest != self.resume_record.permit_digest
            || self.execution_claim.invocation_digest != self.invocation_digest
            || self.execution_claim.transaction_receipt_digest
                != self.resume_record.restored_transaction.receipt_digest
            || self.execution_claim.project_snapshot_root != safepoint.project_snapshot_root
            || self.execution_claim.raw_baseline_identity_digest
                != safepoint.raw_baseline_identity_digest
            || self.execution_claim.raw_baseline_input_digest != safepoint.raw_baseline_input_digest
            || self.execution_claim.raw_decision_view_digest != safepoint.raw_decision_view_digest
            || self.execution_claim.baseline_reasoning_contract_digest
                != safepoint.baseline_reasoning_contract_digest
            || self.execution_claim.reasoning_entry_digest != safepoint.reasoning_entry.digest()?
            || self.execution_claim.predecessor_receipt_head_digest
                != self
                    .resume_record
                    .restoration_claim
                    .successor_receipt_head_digest
            || self.execution_claim.digest()? != self.execution_claim_digest
            || self.verifier_identity_digest
                != self.execution_claim.execution_verifier_identity_digest
            || baseline_execution_receipt_digest(
                self.resume_record.permit_digest,
                self.invocation_digest,
                self.execution_claim_digest,
                self.evidence_digest,
                self.verifier_identity_digest,
            ) != self.receipt_digest
        {
            return Err(deopt_error(
                DeoptimizationFailureCode::CertificateDigestMismatch,
                "baseline execution receipt record does not replay",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DeoptimizationError> {
        let record: Self = decode_canonical(bytes)?;
        record.validate()?;
        Ok(record)
    }
}

fn baseline_execution_receipt_digest(
    resume_permit_digest: Sha256Digest,
    invocation_digest: Sha256Digest,
    execution_claim_digest: Sha256Digest,
    evidence_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
) -> Sha256Digest {
    digest_value(
        EXECUTION_RECEIPT_DOMAIN,
        &json!({
            "evidence_digest": evidence_digest,
            "execution_claim_digest": execution_claim_digest,
            "invocation_digest": invocation_digest,
            "resume_permit_digest": resume_permit_digest,
            "verifier_identity_digest": verifier_identity_digest,
        }),
    )
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeoptimizationFailureCode {
    SchemaVersionMismatch,
    ZeroDigest,
    InvalidResourceReserve,
    InvalidResourceUsage,
    ReasoningEntryMismatch,
    SafepointBindingMismatch,
    TriggerMismatch,
    PlanDigestMismatch,
    TransactionMismatch,
    RestorationMismatch,
    BaselineExecutionMismatch,
    ReceiptChainMismatch,
    ResourceReserveExceeded,
    UnsupportedEvidenceClass,
    EvidencePayloadMismatch,
    VerifierIdentityMismatch,
    CertificateDigestMismatch,
    CanonicalPayloadTooLarge,
    NonCanonicalEncoding,
    ReasoningContract,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeoptimizationError {
    code: DeoptimizationFailureCode,
    detail: String,
}

impl DeoptimizationError {
    pub const fn failure_code(&self) -> DeoptimizationFailureCode {
        self.code
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for DeoptimizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "deoptimization failed ({:?}): {}",
            self.code, self.detail
        )
    }
}

impl Error for DeoptimizationError {}

fn deopt_error(code: DeoptimizationFailureCode, detail: impl Into<String>) -> DeoptimizationError {
    DeoptimizationError {
        code,
        detail: detail.into(),
    }
}

fn reasoning_error(detail: String) -> DeoptimizationError {
    deopt_error(DeoptimizationFailureCode::ReasoningContract, detail)
}

fn json_error(detail: String) -> DeoptimizationError {
    deopt_error(DeoptimizationFailureCode::Json, detail)
}
