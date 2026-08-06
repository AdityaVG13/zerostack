//! Proof-carrying exact deoptimization to a frozen raw-baseline safepoint.
//!
//! Deoptimization is a first-class transition. It restores the complete
//! preregistered effect closure, reinstates the frozen project/reasoning entry,
//! preserves the raw baseline identity, then consumes a linear invocation into
//! a verified baseline execution receipt. A journal-root recovery alone is not
//! baseline readiness or publication authority.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zero_abi::{
    DigestV1, NativeStatePolicyV1, ReasoningContractV1, canonical_json,
    reasoning_contract_digest_v1, sha256,
};
use zero_cert::{CompletenessWitness, Query, VerifiedEvidence};
use zero_store::RecoveryOutcomeV1;

use crate::{
    quality::{QualityAdmissionRecordV1, QualitySelectionV1, quality_envelope_contract_digest_v1},
    recovery::{RecoveryUnknownDecisionV1, dcr_contract_digest_v1},
    semantic_cut::{ReasoningSafepointV1, ReasoningStateStatusV1},
    transaction::{
        RestorationScopeV1, TransactionDispositionV1, TransactionReceiptV1,
        transaction_contract_digest_v1,
    },
    two_phase::{FailureCode, WorkerEnvelope},
};

pub const DEOPTIMIZATION_CONTRACT_VERSION_V1: u16 = 1;
pub const BASELINE_SAFEPOINT_SCHEMA_VERSION_V1: &str = "zerostack.baseline_safepoint.v1";
pub const BASELINE_RESTORATION_SCHEMA_VERSION_V1: &str = "zerostack.baseline_restoration.v1";
pub const BASELINE_EXECUTION_SCHEMA_VERSION_V1: &str = "zerostack.baseline_execution.v1";
pub const DEOPTIMIZATION_PLAN_SCHEMA_VERSION_V1: &str = "zerostack.deoptimization_plan.v1";
pub const DEOPTIMIZATION_RESUME_SCHEMA_SHA256_V1: &str =
    "984eeed082d5a1d190f644072e21b4821c5649f19f240e057dfbb6ff9554e8ba";
pub const DEOPTIMIZATION_EXECUTION_SCHEMA_SHA256_V1: &str =
    "71c51182f265ae08a46f1530778619071a9dddd9d001cc6cb974345fcb450639";
pub const DEOPTIMIZATION_PLAN_SCHEMA_SHA256_V1: &str =
    "667e8ca57b3702882dfb7504b80e13a2de5e6e911e58c8e0d67004e3438da203";
pub const DEOPTIMIZATION_MAX_CANONICAL_BYTES_V1: usize = 1_048_576;

const SAFEPOINT_DOMAIN_V1: &[u8] = b"zerostack.deoptimization.safepoint_claim.v1\0";
const SAFEPOINT_CERTIFICATE_DOMAIN_V1: &[u8] =
    b"zerostack.deoptimization.safepoint_certificate.v1\0";
const REASONING_ENTRY_DOMAIN_V1: &[u8] = b"zerostack.deoptimization.reasoning_entry.v1\0";
const PLAN_DOMAIN_V1: &[u8] = b"zerostack.deoptimization.plan.v1\0";
const RESTORATION_CLAIM_DOMAIN_V1: &[u8] = b"zerostack.deoptimization.restoration_claim.v1\0";
const RESUME_PERMIT_DOMAIN_V1: &[u8] = b"zerostack.deoptimization.resume_permit.v1\0";
const INVOCATION_DOMAIN_V1: &[u8] = b"zerostack.deoptimization.baseline_invocation.v1\0";
const EXECUTION_CLAIM_DOMAIN_V1: &[u8] = b"zerostack.deoptimization.baseline_execution_claim.v1\0";
const EXECUTION_RECEIPT_DOMAIN_V1: &[u8] =
    b"zerostack.deoptimization.baseline_execution_receipt.v1\0";
const VERIFIER_DOMAIN_V1: &[u8] = b"zerostack.deoptimization.verifier_identity.v1\0";
const CONTRACT_DOMAIN_V1: &[u8] = b"zerostack.deoptimization.contract.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RaccWorkV1 {
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

impl RaccWorkV1 {
    fn validate_limit(&self, label: &'static str) -> Result<(), DeoptimizationErrorV1> {
        let observed_input = self
            .uncached_input_tokens
            .checked_add(self.cached_input_tokens)
            .ok_or_else(|| {
                deopt_error(
                    DeoptimizationFailureCodeV1::InvalidResourceReserve,
                    format!("{label} input-token reserve overflows"),
                )
            })?;
        if observed_input != self.logical_input_tokens
            || self.fallback_work == 0
            || self.latency_micros == 0
            || self.peak_memory_bytes == 0
        {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::InvalidResourceReserve,
                format!(
                    "{label} must conserve input tokens and reserve positive fallback work, latency, and peak memory"
                ),
            ));
        }
        Ok(())
    }

    fn validate_usage(&self) -> Result<(), DeoptimizationErrorV1> {
        let observed_input = self
            .uncached_input_tokens
            .checked_add(self.cached_input_tokens)
            .ok_or_else(|| {
                deopt_error(
                    DeoptimizationFailureCodeV1::InvalidResourceUsage,
                    "input-token usage overflows",
                )
            })?;
        if observed_input != self.logical_input_tokens
            || self.fallback_work == 0
            || self.latency_micros == 0
        {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::InvalidResourceUsage,
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
pub struct RouteUsageV1 {
    pub fuel: u64,
    pub elapsed_ms: u64,
    pub io_bytes: u64,
    pub output_bytes: u64,
    pub memory_bytes: u64,
    pub processes: u32,
    pub risk_units: u64,
    pub worker_steps: u64,
    pub work: RaccWorkV1,
}

impl RouteUsageV1 {
    fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        self.work.validate_usage()?;
        if self.elapsed_ms == 0 || self.memory_bytes == 0 || self.worker_steps == 0 {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::InvalidResourceUsage,
                "route usage must charge elapsed time, memory, and worker steps",
            ));
        }
        Ok(())
    }

    fn within(&self, envelope: &WorkerEnvelope, work_limit: &RaccWorkV1) -> bool {
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
pub struct FallbackReserveV1 {
    pub deoptimization_envelope: WorkerEnvelope,
    pub raw_baseline_envelope: WorkerEnvelope,
    pub deoptimization_work_limit: RaccWorkV1,
    pub raw_baseline_work_limit: RaccWorkV1,
}

impl FallbackReserveV1 {
    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
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
) -> Result<(), DeoptimizationErrorV1> {
    if envelope.fuel == 0
        || envelope.deadline_ms == 0
        || envelope.io_bytes == 0
        || envelope.output_bytes == 0
        || envelope.memory_bytes == 0
        || envelope.processes == 0
        || envelope.worker_steps == 0
    {
        return Err(deopt_error(
            DeoptimizationFailureCodeV1::InvalidResourceReserve,
            format!("{label} runtime envelope is not fully reserved"),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "entry_kind")]
pub enum BaselineReasoningEntryV1 {
    ExactNativeContinuation {
        opaque_state_digest: DigestV1,
        parent_response_digest: DigestV1,
        session_identity_digest: DigestV1,
    },
    CanonicalCleanStart {
        clean_start_identity_digest: DigestV1,
    },
}

impl BaselineReasoningEntryV1 {
    fn validate(
        &self,
        contract: &ReasoningContractV1,
        safepoint: &ReasoningSafepointV1,
    ) -> Result<(), DeoptimizationErrorV1> {
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
                    NativeStatePolicyV1::ExactRequired | NativeStatePolicyV1::ExactIfAvailable
                ) || safepoint.reasoning_state_status() != ReasoningStateStatusV1::ExactPreserved
                    || safepoint.opaque_reasoning_state_digest() != *opaque_state_digest.as_bytes()
                {
                    return Err(deopt_error(
                        DeoptimizationFailureCodeV1::ReasoningEntryMismatch,
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
                if contract.native_state_policy() != NativeStatePolicyV1::CleanRestart
                    || safepoint.reasoning_state_status()
                        != ReasoningStateStatusV1::ExactCleanRestart
                    || safepoint.opaque_reasoning_state_digest()
                        != *clean_start_identity_digest.as_bytes()
                {
                    return Err(deopt_error(
                        DeoptimizationFailureCodeV1::ReasoningEntryMismatch,
                        "clean-start identity does not match the frozen baseline contract",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<DigestV1, DeoptimizationErrorV1> {
        Ok(domain_digest(
            REASONING_ENTRY_DOMAIN_V1,
            &canonical_bytes(self)?,
        ))
    }
}

/// Full frozen baseline state. This claim is data until exact verified bytes
/// mint `BaselineSafepointEvidenceV1` before candidate execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineSafepointClaimV1 {
    schema_version: String,
    project_snapshot_root: DigestV1,
    working_tree_scope_digest: DigestV1,
    external_state_inventory_digest: DigestV1,
    comparison_identity_digest: DigestV1,
    raw_baseline_identity_digest: DigestV1,
    raw_baseline_input_digest: DigestV1,
    raw_decision_view_digest: DigestV1,
    assembly_contract_digest: DigestV1,
    raw_worker_contract_digest: DigestV1,
    effect_schema_digest: DigestV1,
    baseline_reasoning_contract: ReasoningContractV1,
    baseline_reasoning_contract_digest: DigestV1,
    reasoning_safepoint: ReasoningSafepointV1,
    reasoning_entry: BaselineReasoningEntryV1,
    sampler_randomness_identity_digest: DigestV1,
    baseline_verifier_identity_digest: DigestV1,
    reserve: FallbackReserveV1,
    transaction_route_digest: DigestV1,
    restoration_route_digest: DigestV1,
    capture_receipt_head_digest: DigestV1,
}

impl BaselineSafepointClaimV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_snapshot_root: DigestV1,
        working_tree_scope_digest: DigestV1,
        external_state_inventory_digest: DigestV1,
        comparison_identity_digest: DigestV1,
        raw_baseline_identity_digest: DigestV1,
        raw_baseline_input_digest: DigestV1,
        raw_decision_view_digest: DigestV1,
        assembly_contract_digest: DigestV1,
        raw_worker_contract_digest: DigestV1,
        effect_schema_digest: DigestV1,
        baseline_reasoning_contract: ReasoningContractV1,
        reasoning_safepoint: ReasoningSafepointV1,
        reasoning_entry: BaselineReasoningEntryV1,
        sampler_randomness_identity_digest: DigestV1,
        baseline_verifier_identity_digest: DigestV1,
        reserve: FallbackReserveV1,
        transaction_route_digest: DigestV1,
        restoration_route_digest: DigestV1,
        capture_receipt_head_digest: DigestV1,
    ) -> Result<Self, DeoptimizationErrorV1> {
        let baseline_reasoning_contract_digest = baseline_reasoning_contract
            .identity_digest()
            .map_err(|error| reasoning_error(error.to_string()))?;
        let claim = Self {
            schema_version: BASELINE_SAFEPOINT_SCHEMA_VERSION_V1.into(),
            project_snapshot_root,
            working_tree_scope_digest,
            external_state_inventory_digest,
            comparison_identity_digest,
            raw_baseline_identity_digest,
            raw_baseline_input_digest,
            raw_decision_view_digest,
            assembly_contract_digest,
            raw_worker_contract_digest,
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

    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        if self.schema_version != BASELINE_SAFEPOINT_SCHEMA_VERSION_V1 {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::SchemaVersionMismatch,
                "baseline safepoint schema version is not v1",
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
                self.raw_worker_contract_digest,
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
                DeoptimizationFailureCodeV1::SafepointBindingMismatch,
                "project, model, reasoning contract, or receipt head differs at safepoint",
            ));
        }
        self.reasoning_entry
            .validate(&self.baseline_reasoning_contract, &self.reasoning_safepoint)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DeoptimizationErrorV1> {
        let claim: Self = decode_canonical(bytes)?;
        claim.validate()?;
        Ok(claim)
    }

    pub fn digest(&self) -> Result<DigestV1, DeoptimizationErrorV1> {
        Ok(domain_digest(SAFEPOINT_DOMAIN_V1, &self.canonical_bytes()?))
    }
}

/// Opaque capture authority. Successful exact verifier output must predate the
/// candidate plan through the frozen receipt-head binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineSafepointEvidenceV1 {
    contract_version: u16,
    claim: BaselineSafepointClaimV1,
    claim_digest: DigestV1,
    evidence_digest: DigestV1,
    verifier_identity_digest: DigestV1,
    certificate_digest: DigestV1,
}

impl BaselineSafepointEvidenceV1 {
    pub fn verify_owner_scoped(
        claim: BaselineSafepointClaimV1,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, DeoptimizationErrorV1> {
        claim.validate()?;
        verify_exact_successful_payload(&claim.canonical_bytes()?, evidence)?;
        let verifier_identity_digest = deoptimization_verifier_identity_v1(evidence);
        if verifier_identity_digest != claim.baseline_verifier_identity_digest {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::VerifierIdentityMismatch,
                "safepoint verifier differs from the frozen verifier route",
            ));
        }
        let claim_digest = claim.digest()?;
        let evidence_digest = verified_evidence_digest(evidence)?;
        let certificate_digest = digest_value(
            SAFEPOINT_CERTIFICATE_DOMAIN_V1,
            &json!({
                "claim_digest": claim_digest,
                "contract_version": DEOPTIMIZATION_CONTRACT_VERSION_V1,
                "evidence_digest": evidence_digest,
                "verifier_identity_digest": verifier_identity_digest,
            }),
        );
        Ok(Self {
            contract_version: DEOPTIMIZATION_CONTRACT_VERSION_V1,
            claim,
            claim_digest,
            evidence_digest,
            verifier_identity_digest,
            certificate_digest,
        })
    }

    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION_V1 {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::SchemaVersionMismatch,
                "safepoint certificate contract version is not v1",
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
            SAFEPOINT_CERTIFICATE_DOMAIN_V1,
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
                DeoptimizationFailureCodeV1::CertificateDigestMismatch,
                "safepoint certificate does not bind its claim and proof",
            ));
        }
        Ok(())
    }

    pub fn record(&self) -> BaselineSafepointCertificateRecordV1 {
        BaselineSafepointCertificateRecordV1 {
            contract_version: self.contract_version,
            claim: self.claim.clone(),
            claim_digest: self.claim_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            certificate_digest: self.certificate_digest,
        }
    }

    pub const fn claim(&self) -> &BaselineSafepointClaimV1 {
        &self.claim
    }
    pub const fn certificate_digest(&self) -> DigestV1 {
        self.certificate_digest
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineSafepointCertificateRecordV1 {
    pub contract_version: u16,
    pub claim: BaselineSafepointClaimV1,
    pub claim_digest: DigestV1,
    pub evidence_digest: DigestV1,
    pub verifier_identity_digest: DigestV1,
    pub certificate_digest: DigestV1,
}

impl BaselineSafepointCertificateRecordV1 {
    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        BaselineSafepointEvidenceV1 {
            contract_version: self.contract_version,
            claim: self.claim.clone(),
            claim_digest: self.claim_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            certificate_digest: self.certificate_digest,
        }
        .validate()
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "trigger_kind")]
pub enum DeoptimizationTriggerV1 {
    RecoveryUnknown {
        problem_digest: DigestV1,
        decision_digest: DigestV1,
    },
    QualityBaselineSelection {
        quality_admission_digest: DigestV1,
    },
    FailClosed {
        failure_code: FailureCode,
        failure_receipt_digest: DigestV1,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeoptimizationPlanClaimV1 {
    schema_version: String,
    safepoint_certificate_digest: DigestV1,
    trigger: DeoptimizationTriggerV1,
    candidate_action_digest: DigestV1,
    candidate_state_digest: DigestV1,
    candidate_closure_manifest_digest: DigestV1,
    prior_work_receipt_digest: DigestV1,
    kernel_binding_digest: DigestV1,
    kernel_admission_digest: DigestV1,
    plan_digest: DigestV1,
}

impl DeoptimizationPlanClaimV1 {
    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        if self.schema_version != DEOPTIMIZATION_PLAN_SCHEMA_VERSION_V1 {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::SchemaVersionMismatch,
                "deoptimization plan claim schema version is not v1",
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
            PLAN_DOMAIN_V1,
            &json!({
                "candidate_action_digest": self.candidate_action_digest,
                "candidate_closure_manifest_digest": self.candidate_closure_manifest_digest,
                "candidate_state_digest": self.candidate_state_digest,
                "contract_version": DEOPTIMIZATION_CONTRACT_VERSION_V1,
                "kernel_admission_digest": self.kernel_admission_digest,
                "kernel_binding_digest": self.kernel_binding_digest,
                "prior_work_receipt_digest": self.prior_work_receipt_digest,
                "safepoint_certificate_digest": self.safepoint_certificate_digest,
                "trigger": self.trigger,
            }),
        );
        if expected != self.plan_digest {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::PlanDigestMismatch,
                "deoptimization plan claim does not replay",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DeoptimizationErrorV1> {
        let claim: Self = decode_canonical(bytes)?;
        claim.validate()?;
        Ok(claim)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeoptimizationPlanV1 {
    contract_version: u16,
    safepoint: BaselineSafepointEvidenceV1,
    trigger: DeoptimizationTriggerV1,
    candidate_action_digest: DigestV1,
    candidate_state_digest: DigestV1,
    candidate_closure_manifest_digest: DigestV1,
    prior_work_receipt_digest: DigestV1,
    kernel_binding_digest: DigestV1,
    kernel_admission_digest: DigestV1,
    plan_digest: DigestV1,
}

impl DeoptimizationPlanV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn for_recovery_unknown(
        safepoint: BaselineSafepointEvidenceV1,
        unknown: &RecoveryUnknownDecisionV1,
        candidate_action_digest: DigestV1,
        candidate_state_digest: DigestV1,
        candidate_closure_manifest_digest: DigestV1,
        prior_work_receipt_digest: DigestV1,
        kernel_binding_digest: DigestV1,
        kernel_admission_digest: DigestV1,
    ) -> Result<Self, DeoptimizationErrorV1> {
        if !unknown.raw_baseline_required()
            || unknown.fallback_safepoint() != safepoint.certificate_digest()
        {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::TriggerMismatch,
                "DCR Unknown does not require this frozen safepoint",
            ));
        }
        Self::new(
            safepoint,
            DeoptimizationTriggerV1::RecoveryUnknown {
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
        safepoint: BaselineSafepointEvidenceV1,
        admission: &QualityAdmissionRecordV1,
        candidate_action_digest: DigestV1,
        candidate_state_digest: DigestV1,
        candidate_closure_manifest_digest: DigestV1,
        prior_work_receipt_digest: DigestV1,
        kernel_binding_digest: DigestV1,
        kernel_admission_digest: DigestV1,
    ) -> Result<Self, DeoptimizationErrorV1> {
        admission.validate().map_err(|error| {
            deopt_error(
                DeoptimizationFailureCodeV1::TriggerMismatch,
                format!("quality admission is invalid: {error}"),
            )
        })?;
        if admission.selection != QualitySelectionV1::FrozenBaseline
            || admission.raw_baseline_identity_digest
                != safepoint.claim.raw_baseline_identity_digest
        {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::TriggerMismatch,
                "quality admission does not select the frozen safepoint baseline",
            ));
        }
        Self::new(
            safepoint,
            DeoptimizationTriggerV1::QualityBaselineSelection {
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
        safepoint: BaselineSafepointEvidenceV1,
        failure_code: FailureCode,
        failure_receipt_digest: DigestV1,
        candidate_action_digest: DigestV1,
        candidate_state_digest: DigestV1,
        candidate_closure_manifest_digest: DigestV1,
        prior_work_receipt_digest: DigestV1,
        kernel_binding_digest: DigestV1,
        kernel_admission_digest: DigestV1,
    ) -> Result<Self, DeoptimizationErrorV1> {
        require_nonzero("failure receipt", &[failure_receipt_digest])?;
        Self::new(
            safepoint,
            DeoptimizationTriggerV1::FailClosed {
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
        safepoint: BaselineSafepointEvidenceV1,
        trigger: DeoptimizationTriggerV1,
        candidate_action_digest: DigestV1,
        candidate_state_digest: DigestV1,
        candidate_closure_manifest_digest: DigestV1,
        prior_work_receipt_digest: DigestV1,
        kernel_binding_digest: DigestV1,
        kernel_admission_digest: DigestV1,
    ) -> Result<Self, DeoptimizationErrorV1> {
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
            contract_version: DEOPTIMIZATION_CONTRACT_VERSION_V1,
            safepoint,
            trigger,
            candidate_action_digest,
            candidate_state_digest,
            candidate_closure_manifest_digest,
            prior_work_receipt_digest,
            kernel_binding_digest,
            kernel_admission_digest,
            plan_digest: DigestV1::ZERO,
        };
        plan.plan_digest = plan.expected_digest()?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION_V1 {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::SchemaVersionMismatch,
                "deoptimization plan contract version is not v1",
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
                DeoptimizationFailureCodeV1::PlanDigestMismatch,
                "deoptimization plan digest does not bind its trigger and candidate attempt",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<DigestV1, DeoptimizationErrorV1> {
        Ok(digest_value(
            PLAN_DOMAIN_V1,
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

    pub fn record(&self) -> DeoptimizationPlanRecordV1 {
        DeoptimizationPlanRecordV1 {
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

    pub fn claim(&self) -> DeoptimizationPlanClaimV1 {
        DeoptimizationPlanClaimV1 {
            schema_version: DEOPTIMIZATION_PLAN_SCHEMA_VERSION_V1.into(),
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

    pub const fn plan_digest(&self) -> DigestV1 {
        self.plan_digest
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeoptimizationPlanRecordV1 {
    pub contract_version: u16,
    pub safepoint: BaselineSafepointCertificateRecordV1,
    pub trigger: DeoptimizationTriggerV1,
    pub candidate_action_digest: DigestV1,
    pub candidate_state_digest: DigestV1,
    pub candidate_closure_manifest_digest: DigestV1,
    pub prior_work_receipt_digest: DigestV1,
    pub kernel_binding_digest: DigestV1,
    pub kernel_admission_digest: DigestV1,
    pub plan_digest: DigestV1,
}

impl DeoptimizationPlanRecordV1 {
    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        self.safepoint.validate()?;
        self.claim().validate()?;
        let expected = digest_value(
            PLAN_DOMAIN_V1,
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
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION_V1
            || expected != self.plan_digest
        {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::PlanDigestMismatch,
                "deoptimization plan record does not replay",
            ));
        }
        Ok(())
    }

    pub fn claim(&self) -> DeoptimizationPlanClaimV1 {
        DeoptimizationPlanClaimV1 {
            schema_version: DEOPTIMIZATION_PLAN_SCHEMA_VERSION_V1.into(),
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
pub struct BaselineRestorationClaimV1 {
    schema_version: String,
    plan_digest: DigestV1,
    safepoint_certificate_digest: DigestV1,
    transaction_receipt_digest: DigestV1,
    restored_project_root: DigestV1,
    restored_external_inventory_digest: DigestV1,
    restored_reasoning_contract_digest: DigestV1,
    restored_fixed_model_digest: DigestV1,
    restored_reasoning_entry_digest: DigestV1,
    raw_baseline_identity_digest: DigestV1,
    raw_baseline_input_digest: DigestV1,
    raw_decision_view_digest: DigestV1,
    candidate_overlay_disposition_digest: DigestV1,
    visible_buffer_disposition_digest: DigestV1,
    prior_receipt_head_digest: DigestV1,
    successor_receipt_head_digest: DigestV1,
    restoration_verifier_identity_digest: DigestV1,
    deoptimization_usage: RouteUsageV1,
}

impl BaselineRestorationClaimV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plan: &DeoptimizationPlanV1,
        transaction_receipt_digest: DigestV1,
        candidate_overlay_disposition_digest: DigestV1,
        visible_buffer_disposition_digest: DigestV1,
        successor_receipt_head_digest: DigestV1,
        restoration_verifier_identity_digest: DigestV1,
        deoptimization_usage: RouteUsageV1,
    ) -> Result<Self, DeoptimizationErrorV1> {
        let safepoint = plan.safepoint.claim();
        let claim = Self {
            schema_version: BASELINE_RESTORATION_SCHEMA_VERSION_V1.into(),
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

    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        if self.schema_version != BASELINE_RESTORATION_SCHEMA_VERSION_V1 {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::SchemaVersionMismatch,
                "baseline restoration schema version is not v1",
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
                DeoptimizationFailureCodeV1::ReceiptChainMismatch,
                "restoration must advance the receipt head",
            ));
        }
        self.deoptimization_usage.validate()
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DeoptimizationErrorV1> {
        let claim: Self = decode_canonical(bytes)?;
        claim.validate()?;
        Ok(claim)
    }

    pub fn digest(&self) -> Result<DigestV1, DeoptimizationErrorV1> {
        Ok(domain_digest(
            RESTORATION_CLAIM_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestoredTransactionRecordV1 {
    pub receipt_digest: DigestV1,
    pub action_digest: DigestV1,
    pub closure_manifest_digest: DigestV1,
    pub baseline_state: DigestV1,
    pub candidate_state: DigestV1,
    pub external_inventory_digest: DigestV1,
    pub resource_count: u16,
    pub external_resource_count: u16,
    pub recovery_outcome: RecoveryOutcomeV1,
}

/// Opaque restoration authority. It can mint one frozen baseline invocation,
/// but it cannot enter G8 or claim baseline execution or publication.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineResumePermitV1 {
    contract_version: u16,
    plan: DeoptimizationPlanV1,
    restoration_claim: BaselineRestorationClaimV1,
    restored_transaction: RestoredTransactionRecordV1,
    restoration_claim_digest: DigestV1,
    evidence_digest: DigestV1,
    verifier_identity_digest: DigestV1,
    permit_digest: DigestV1,
}

impl BaselineResumePermitV1 {
    pub fn verify_restoration(
        plan: DeoptimizationPlanV1,
        transaction: TransactionReceiptV1,
        restoration_claim: BaselineRestorationClaimV1,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, DeoptimizationErrorV1> {
        plan.validate()?;
        restoration_claim.validate()?;
        transaction.canonical_bytes().map_err(|error| {
            deopt_error(
                DeoptimizationFailureCodeV1::TransactionMismatch,
                format!("transaction receipt is invalid: {error}"),
            )
        })?;
        let safepoint = plan.safepoint.claim();
        if transaction.disposition() != TransactionDispositionV1::BaselineRootRecovered
            || transaction.restoration_scope() != RestorationScopeV1::DeclaredEffectClosure
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
                DeoptimizationFailureCodeV1::TransactionMismatch,
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
                DeoptimizationFailureCodeV1::RestorationMismatch,
                "restoration proof differs from the frozen safepoint",
            ));
        }
        if !restoration_claim.deoptimization_usage.within(
            &safepoint.reserve.deoptimization_envelope,
            &safepoint.reserve.deoptimization_work_limit,
        ) {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::ResourceReserveExceeded,
                "deoptimization work exceeds the frozen fallback reserve",
            ));
        }
        verify_exact_successful_payload(&restoration_claim.canonical_bytes()?, evidence)?;
        let verifier_identity_digest = deoptimization_verifier_identity_v1(evidence);
        if verifier_identity_digest != restoration_claim.restoration_verifier_identity_digest {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::VerifierIdentityMismatch,
                "restoration verifier differs from the claim route",
            ));
        }
        let restored_transaction = RestoredTransactionRecordV1 {
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
            contract_version: DEOPTIMIZATION_CONTRACT_VERSION_V1,
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

    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION_V1 {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::SchemaVersionMismatch,
                "baseline resume permit contract version is not v1",
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
                DeoptimizationFailureCodeV1::CertificateDigestMismatch,
                "baseline resume permit does not replay against its route evidence",
            ));
        }
        Ok(())
    }

    pub fn into_invocation(self) -> Result<FrozenBaselineInvocationV1, DeoptimizationErrorV1> {
        self.validate()?;
        let resume_record = self.record();
        let safepoint = self.plan.safepoint.claim();
        let mut invocation = FrozenBaselineInvocationV1 {
            resume_record,
            contract_version: DEOPTIMIZATION_CONTRACT_VERSION_V1,
            resume_permit_digest: self.permit_digest,
            predecessor_receipt_head_digest: self.restoration_claim.successor_receipt_head_digest,
            transaction_receipt_digest: self.restored_transaction.receipt_digest,
            project_snapshot_root: safepoint.project_snapshot_root,
            raw_baseline_identity_digest: safepoint.raw_baseline_identity_digest,
            raw_baseline_input_digest: safepoint.raw_baseline_input_digest,
            raw_decision_view_digest: safepoint.raw_decision_view_digest,
            comparison_identity_digest: safepoint.comparison_identity_digest,
            assembly_contract_digest: safepoint.assembly_contract_digest,
            raw_worker_contract_digest: safepoint.raw_worker_contract_digest,
            effect_schema_digest: safepoint.effect_schema_digest,
            baseline_reasoning_contract: safepoint.baseline_reasoning_contract.clone(),
            baseline_reasoning_contract_digest: safepoint.baseline_reasoning_contract_digest,
            reasoning_entry: safepoint.reasoning_entry.clone(),
            sampler_randomness_identity_digest: safepoint.sampler_randomness_identity_digest,
            baseline_verifier_identity_digest: safepoint.baseline_verifier_identity_digest,
            raw_baseline_envelope: safepoint.reserve.raw_baseline_envelope,
            raw_baseline_work_limit: safepoint.reserve.raw_baseline_work_limit,
            invocation_digest: DigestV1::ZERO,
        };
        invocation.invocation_digest = invocation.expected_digest()?;
        invocation.validate()?;
        Ok(invocation)
    }

    pub fn record(&self) -> BaselineResumeReceiptRecordV1 {
        BaselineResumeReceiptRecordV1 {
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

    pub const fn permit_digest(&self) -> DigestV1 {
        self.permit_digest
    }
    pub const fn restored_transaction(&self) -> &RestoredTransactionRecordV1 {
        &self.restored_transaction
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineResumeReceiptRecordV1 {
    pub contract_version: u16,
    pub plan: DeoptimizationPlanRecordV1,
    pub restoration_claim: BaselineRestorationClaimV1,
    pub restored_transaction: RestoredTransactionRecordV1,
    pub restoration_claim_digest: DigestV1,
    pub evidence_digest: DigestV1,
    pub verifier_identity_digest: DigestV1,
    pub permit_digest: DigestV1,
}

impl BaselineResumeReceiptRecordV1 {
    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        self.plan.validate()?;
        self.restoration_claim.validate()?;
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION_V1
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
                DeoptimizationFailureCodeV1::CertificateDigestMismatch,
                "baseline resume record does not replay",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DeoptimizationErrorV1> {
        let record: Self = decode_canonical(bytes)?;
        record.validate()?;
        Ok(record)
    }
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenBaselineInvocationV1 {
    resume_record: BaselineResumeReceiptRecordV1,
    contract_version: u16,
    resume_permit_digest: DigestV1,
    predecessor_receipt_head_digest: DigestV1,
    transaction_receipt_digest: DigestV1,
    project_snapshot_root: DigestV1,
    raw_baseline_identity_digest: DigestV1,
    raw_baseline_input_digest: DigestV1,
    raw_decision_view_digest: DigestV1,
    comparison_identity_digest: DigestV1,
    assembly_contract_digest: DigestV1,
    raw_worker_contract_digest: DigestV1,
    effect_schema_digest: DigestV1,
    baseline_reasoning_contract: ReasoningContractV1,
    baseline_reasoning_contract_digest: DigestV1,
    reasoning_entry: BaselineReasoningEntryV1,
    sampler_randomness_identity_digest: DigestV1,
    baseline_verifier_identity_digest: DigestV1,
    raw_baseline_envelope: WorkerEnvelope,
    raw_baseline_work_limit: RaccWorkV1,
    invocation_digest: DigestV1,
}

impl FrozenBaselineInvocationV1 {
    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION_V1 {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::SchemaVersionMismatch,
                "frozen baseline invocation contract version is not v1",
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
                DeoptimizationFailureCodeV1::SafepointBindingMismatch,
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
                self.raw_worker_contract_digest,
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
                DeoptimizationFailureCodeV1::CertificateDigestMismatch,
                "frozen baseline invocation does not bind its exact contract",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<DigestV1, DeoptimizationErrorV1> {
        Ok(digest_value(
            INVOCATION_DOMAIN_V1,
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
                "raw_worker_contract_digest": self.raw_worker_contract_digest,
                "reasoning_entry": self.reasoning_entry,
                "resume_permit_digest": self.resume_permit_digest,
                "sampler_randomness_identity_digest": self.sampler_randomness_identity_digest,
                "transaction_receipt_digest": self.transaction_receipt_digest,
            }),
        ))
    }

    pub const fn invocation_digest(&self) -> DigestV1 {
        self.invocation_digest
    }
    pub const fn raw_baseline_identity_digest(&self) -> DigestV1 {
        self.raw_baseline_identity_digest
    }
    pub const fn project_snapshot_root(&self) -> DigestV1 {
        self.project_snapshot_root
    }
}

fn baseline_invocation_digest_from_resume_record(
    record: &BaselineResumeReceiptRecordV1,
) -> Result<DigestV1, DeoptimizationErrorV1> {
    record.validate()?;
    let safepoint = &record.plan.safepoint.claim;
    Ok(digest_value(
        INVOCATION_DOMAIN_V1,
        &json!({
            "assembly_contract_digest": safepoint.assembly_contract_digest,
            "baseline_reasoning_contract": safepoint.baseline_reasoning_contract,
            "baseline_reasoning_contract_digest": safepoint.baseline_reasoning_contract_digest,
            "baseline_verifier_identity_digest": safepoint.baseline_verifier_identity_digest,
            "comparison_identity_digest": safepoint.comparison_identity_digest,
            "contract_version": DEOPTIMIZATION_CONTRACT_VERSION_V1,
            "effect_schema_digest": safepoint.effect_schema_digest,
            "predecessor_receipt_head_digest": record.restoration_claim.successor_receipt_head_digest,
            "project_snapshot_root": safepoint.project_snapshot_root,
            "raw_baseline_envelope": safepoint.reserve.raw_baseline_envelope,
            "raw_baseline_identity_digest": safepoint.raw_baseline_identity_digest,
            "raw_baseline_input_digest": safepoint.raw_baseline_input_digest,
            "raw_baseline_work_limit": safepoint.reserve.raw_baseline_work_limit,
            "raw_decision_view_digest": safepoint.raw_decision_view_digest,
            "raw_worker_contract_digest": safepoint.raw_worker_contract_digest,
            "reasoning_entry": safepoint.reasoning_entry,
            "resume_permit_digest": record.permit_digest,
            "sampler_randomness_identity_digest": safepoint.sampler_randomness_identity_digest,
            "transaction_receipt_digest": record.restored_transaction.receipt_digest,
        }),
    ))
}

fn resume_permit_digest(
    plan_digest: DigestV1,
    restoration_claim_digest: DigestV1,
    transaction_receipt_digest: DigestV1,
    evidence_digest: DigestV1,
    verifier_identity_digest: DigestV1,
) -> DigestV1 {
    digest_value(
        RESUME_PERMIT_DOMAIN_V1,
        &json!({
            "contract_version": DEOPTIMIZATION_CONTRACT_VERSION_V1,
            "evidence_digest": evidence_digest,
            "plan_digest": plan_digest,
            "restoration_claim_digest": restoration_claim_digest,
            "transaction_receipt_digest": transaction_receipt_digest,
            "verifier_identity_digest": verifier_identity_digest,
        }),
    )
}

pub fn deoptimization_verifier_identity_v1(evidence: &VerifiedEvidence<'_, '_>) -> DigestV1 {
    let provenance = evidence.provenance();
    digest_value(
        VERIFIER_DOMAIN_V1,
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
) -> Result<(), DeoptimizationErrorV1> {
    match (evidence.query(), &evidence.certificate().completeness) {
        (Query::BuildReceipt { .. }, CompletenessWitness::BuildReceipt { exit_code: 0, .. })
        | (Query::TestTrace { .. }, CompletenessWitness::TestTrace { exit_code: 0, .. }) => {}
        _ => {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::UnsupportedEvidenceClass,
                "deoptimization requires a successful verified build or test trace",
            ));
        }
    }
    if evidence.payload() != expected {
        return Err(deopt_error(
            DeoptimizationFailureCodeV1::EvidencePayloadMismatch,
            "verified evidence payload differs from the exact canonical claim",
        ));
    }
    Ok(())
}

fn verified_evidence_digest(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<DigestV1, DeoptimizationErrorV1> {
    Ok(DigestV1::from_bytes(
        evidence
            .certificate()
            .canonical_digest()
            .map_err(|error| json_error(error.to_string()))?,
    ))
}

pub fn deoptimization_contract_manifest_v1() -> Value {
    json!({
        "baseline_entry_modes": ["exact_native_continuation", "canonical_clean_start"],
        "baseline_invocation_authority": "linear_resume_permit_to_invocation_then_verified_execution_receipt",
        "canonical_encoding": "sorted_key_json_no_whitespace",
        "contract_version": DEOPTIMIZATION_CONTRACT_VERSION_V1,
        "deoptimization_triggers": ["recovery_unknown", "quality_baseline_selection", "fail_closed"],
        "linked_contracts": {
            "dominance_complete_recovery": dcr_contract_digest_v1(),
            "quality_envelope": quality_envelope_contract_digest_v1(),
            "reasoning_contract": reasoning_contract_digest_v1(),
            "transaction": transaction_contract_digest_v1(),
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
        "max_canonical_bytes": DEOPTIMIZATION_MAX_CANONICAL_BYTES_V1,
        "proof_carrier": "zero_cert::VerifiedEvidence_successful_build_or_test_exact_claim_payload",
        "published_execution_schema_sha256": DEOPTIMIZATION_EXECUTION_SCHEMA_SHA256_V1,
        "published_plan_schema_sha256": DEOPTIMIZATION_PLAN_SCHEMA_SHA256_V1,
        "published_resume_schema_sha256": DEOPTIMIZATION_RESUME_SCHEMA_SHA256_V1,
        "resource_arithmetic": "integer_native_coordinates_no_scalar_laundering",
    })
}

pub fn deoptimization_contract_digest_v1() -> DigestV1 {
    digest_value(CONTRACT_DOMAIN_V1, &deoptimization_contract_manifest_v1())
}

fn canonical_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, DeoptimizationErrorV1> {
    let value = serde_json::to_value(value).map_err(|error| json_error(error.to_string()))?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() > DEOPTIMIZATION_MAX_CANONICAL_BYTES_V1 {
        return Err(deopt_error(
            DeoptimizationFailureCodeV1::CanonicalPayloadTooLarge,
            "deoptimization payload exceeds its canonical byte bound",
        ));
    }
    Ok(bytes)
}

fn decode_canonical<T>(bytes: &[u8]) -> Result<T, DeoptimizationErrorV1>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    if bytes.len() > DEOPTIMIZATION_MAX_CANONICAL_BYTES_V1 {
        return Err(deopt_error(
            DeoptimizationFailureCodeV1::CanonicalPayloadTooLarge,
            "deoptimization payload exceeds its canonical byte bound",
        ));
    }
    let value = serde_json::from_slice(bytes).map_err(|error| json_error(error.to_string()))?;
    if canonical_bytes(&value)? != bytes {
        return Err(deopt_error(
            DeoptimizationFailureCodeV1::NonCanonicalEncoding,
            "deoptimization bytes are not canonical sorted-key JSON",
        ));
    }
    Ok(value)
}

fn digest_value(domain: &[u8], value: &Value) -> DigestV1 {
    let canonical = canonical_json(value);
    let mut bytes = Vec::with_capacity(domain.len() + canonical.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(canonical.as_bytes());
    DigestV1::from_bytes(sha256(&bytes))
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> DigestV1 {
    let mut value = Vec::with_capacity(domain.len() + bytes.len());
    value.extend_from_slice(domain);
    value.extend_from_slice(bytes);
    DigestV1::from_bytes(sha256(&value))
}

fn require_nonzero(label: &'static str, values: &[DigestV1]) -> Result<(), DeoptimizationErrorV1> {
    if values.iter().any(|value| *value == DigestV1::ZERO) {
        Err(deopt_error(
            DeoptimizationFailureCodeV1::ZeroDigest,
            format!("{label} contains a zero digest"),
        ))
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineExecutionClaimV1 {
    schema_version: String,
    invocation_digest: DigestV1,
    resume_permit_digest: DigestV1,
    transaction_receipt_digest: DigestV1,
    project_snapshot_root: DigestV1,
    raw_baseline_identity_digest: DigestV1,
    raw_baseline_input_digest: DigestV1,
    raw_decision_view_digest: DigestV1,
    baseline_reasoning_contract_digest: DigestV1,
    reasoning_entry_digest: DigestV1,
    predecessor_receipt_head_digest: DigestV1,
    output_digest: DigestV1,
    effects_digest: DigestV1,
    baseline_action_digest: DigestV1,
    baseline_acceptance_digest: DigestV1,
    baseline_successor_root: DigestV1,
    baseline_transaction_receipt_digest: DigestV1,
    raw_baseline_usage: RouteUsageV1,
    successor_receipt_head_digest: DigestV1,
    execution_verifier_identity_digest: DigestV1,
}

impl BaselineExecutionClaimV1 {
    pub fn new(
        invocation: &FrozenBaselineInvocationV1,
        output_digest: DigestV1,
        effects_digest: DigestV1,
        baseline_action_digest: DigestV1,
        baseline_acceptance_digest: DigestV1,
        baseline_successor_root: DigestV1,
        baseline_transaction_receipt_digest: DigestV1,
        raw_baseline_usage: RouteUsageV1,
        successor_receipt_head_digest: DigestV1,
        execution_verifier_identity_digest: DigestV1,
    ) -> Result<Self, DeoptimizationErrorV1> {
        invocation.validate()?;
        let claim = Self {
            schema_version: BASELINE_EXECUTION_SCHEMA_VERSION_V1.into(),
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
                DeoptimizationFailureCodeV1::ResourceReserveExceeded,
                "raw baseline execution exceeds its frozen reserve",
            ));
        }
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        if self.schema_version != BASELINE_EXECUTION_SCHEMA_VERSION_V1 {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::SchemaVersionMismatch,
                "baseline execution claim schema version is not v1",
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
                DeoptimizationFailureCodeV1::ReceiptChainMismatch,
                "baseline execution did not advance the receipt chain",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DeoptimizationErrorV1> {
        let claim: Self = decode_canonical(bytes)?;
        claim.validate()?;
        Ok(claim)
    }

    pub fn digest(&self) -> Result<DigestV1, DeoptimizationErrorV1> {
        Ok(domain_digest(
            EXECUTION_CLAIM_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

/// Opaque proof that the exact frozen raw baseline completed successfully.
/// It authorizes G8 fallback closure, not publication or native durability alone.
#[derive(Debug)]
pub struct BaselineExecutionReceiptV1 {
    resume_record: BaselineResumeReceiptRecordV1,
    invocation_digest: DigestV1,
    execution_claim: BaselineExecutionClaimV1,
    execution_claim_digest: DigestV1,
    evidence_digest: DigestV1,
    verifier_identity_digest: DigestV1,
    receipt_digest: DigestV1,
}

impl BaselineExecutionReceiptV1 {
    pub fn verify_execution(
        invocation: FrozenBaselineInvocationV1,
        execution_claim: BaselineExecutionClaimV1,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, DeoptimizationErrorV1> {
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
                DeoptimizationFailureCodeV1::BaselineExecutionMismatch,
                "baseline execution differs from the frozen invocation",
            ));
        }
        if !execution_claim.raw_baseline_usage.within(
            &invocation.raw_baseline_envelope,
            &invocation.raw_baseline_work_limit,
        ) {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::ResourceReserveExceeded,
                "raw baseline execution exceeds its frozen reserve",
            ));
        }
        verify_exact_successful_payload(&execution_claim.canonical_bytes()?, evidence)?;
        let verifier_identity_digest = deoptimization_verifier_identity_v1(evidence);
        if verifier_identity_digest != execution_claim.execution_verifier_identity_digest {
            return Err(deopt_error(
                DeoptimizationFailureCodeV1::VerifierIdentityMismatch,
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

    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        self.record().validate()
    }

    pub fn record(&self) -> BaselineExecutionReceiptRecordV1 {
        BaselineExecutionReceiptRecordV1 {
            contract_version: DEOPTIMIZATION_CONTRACT_VERSION_V1,
            resume_record: self.resume_record.clone(),
            invocation_digest: self.invocation_digest,
            execution_claim: self.execution_claim.clone(),
            execution_claim_digest: self.execution_claim_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            receipt_digest: self.receipt_digest,
        }
    }

    pub const fn receipt_digest(&self) -> DigestV1 {
        self.receipt_digest
    }

    pub const fn restored_transaction(&self) -> &RestoredTransactionRecordV1 {
        &self.resume_record.restored_transaction
    }

    pub const fn project_snapshot_root(&self) -> DigestV1 {
        self.resume_record
            .plan
            .safepoint
            .claim
            .project_snapshot_root
    }

    pub const fn baseline_successor_root(&self) -> DigestV1 {
        self.execution_claim.baseline_successor_root
    }

    pub const fn baseline_transaction_receipt_digest(&self) -> DigestV1 {
        self.execution_claim.baseline_transaction_receipt_digest
    }

    pub const fn baseline_action_digest(&self) -> DigestV1 {
        self.execution_claim.baseline_action_digest
    }

    pub const fn baseline_acceptance_digest(&self) -> DigestV1 {
        self.execution_claim.baseline_acceptance_digest
    }

    pub const fn kernel_binding_digest(&self) -> DigestV1 {
        self.resume_record.plan.kernel_binding_digest
    }

    pub const fn kernel_admission_digest(&self) -> DigestV1 {
        self.resume_record.plan.kernel_admission_digest
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineExecutionReceiptRecordV1 {
    pub contract_version: u16,
    pub resume_record: BaselineResumeReceiptRecordV1,
    pub invocation_digest: DigestV1,
    pub execution_claim: BaselineExecutionClaimV1,
    pub execution_claim_digest: DigestV1,
    pub evidence_digest: DigestV1,
    pub verifier_identity_digest: DigestV1,
    pub receipt_digest: DigestV1,
}

impl BaselineExecutionReceiptRecordV1 {
    pub fn validate(&self) -> Result<(), DeoptimizationErrorV1> {
        self.resume_record.validate()?;
        self.execution_claim.validate()?;
        let safepoint = &self.resume_record.plan.safepoint.claim;
        if self.contract_version != DEOPTIMIZATION_CONTRACT_VERSION_V1
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
                DeoptimizationFailureCodeV1::CertificateDigestMismatch,
                "baseline execution receipt record does not replay",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DeoptimizationErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DeoptimizationErrorV1> {
        let record: Self = decode_canonical(bytes)?;
        record.validate()?;
        Ok(record)
    }
}

fn baseline_execution_receipt_digest(
    resume_permit_digest: DigestV1,
    invocation_digest: DigestV1,
    execution_claim_digest: DigestV1,
    evidence_digest: DigestV1,
    verifier_identity_digest: DigestV1,
) -> DigestV1 {
    digest_value(
        EXECUTION_RECEIPT_DOMAIN_V1,
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
pub enum DeoptimizationFailureCodeV1 {
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
pub struct DeoptimizationErrorV1 {
    code: DeoptimizationFailureCodeV1,
    detail: String,
}

impl DeoptimizationErrorV1 {
    pub const fn failure_code(&self) -> DeoptimizationFailureCodeV1 {
        self.code
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for DeoptimizationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "deoptimization failed ({:?}): {}",
            self.code, self.detail
        )
    }
}

impl Error for DeoptimizationErrorV1 {}

fn deopt_error(
    code: DeoptimizationFailureCodeV1,
    detail: impl Into<String>,
) -> DeoptimizationErrorV1 {
    DeoptimizationErrorV1 {
        code,
        detail: detail.into(),
    }
}

fn reasoning_error(detail: String) -> DeoptimizationErrorV1 {
    deopt_error(DeoptimizationFailureCodeV1::ReasoningContract, detail)
}

fn json_error(detail: String) -> DeoptimizationErrorV1 {
    deopt_error(DeoptimizationFailureCodeV1::Json, detail)
}

#[cfg(test)]
mod tests {
    use std::{
        borrow::Cow,
        collections::{BTreeMap, BTreeSet},
    };

    use super::*;
    use crate::{
        transaction::{
            EffectClosureManifestV1, EffectClosureRequestV1, EffectResourceClosureV1,
            ResourceIsolationModeV1, ResourceRestorationModeV1, TransactionAccessV1,
            TransactionResourceKindV1, TransactionResourceRequirementV1,
            begin_effect_transaction_v1, effect_journal_binding_v1, validate_effect_closure_v1,
        },
        two_phase::{ClosureKind, TransactionClosure},
    };
    use tempfile::tempdir;
    use zero_abi::{
        ArtifactOwnerV1, CwirVerifierClassV1, EffectProgramV1, EffectRollbackV1, EffectTargetV1,
        EffectVerificationPlanV1, EffectVerificationStepV1, TypedEffectOperationV1,
    };
    use zero_cert::{
        EvidenceCertificate, ObjectId, OperatorLock, Provenance, Resolver, SpanRef, TestId, verify,
    };
    use zero_store::{DurableProfileIdV1, JournalPathsV1, initialize_published_root_v1};

    fn d(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn verifier_route() -> DigestV1 {
        digest_value(
            VERIFIER_DOMAIN_V1,
            &json!({
                "index_id": "deopt-index",
                "index_version": "1",
                "operator_id": "deopt-verifier",
                "operator_version": "1",
                "parser_id": "deopt-parser",
                "parser_version": "1",
            }),
        )
    }

    fn reasoning_contract(policy: NativeStatePolicyV1) -> ReasoningContractV1 {
        ReasoningContractV1::new(
            d(5),
            d(6),
            d(7),
            d(8),
            d(9),
            "enabled",
            "high",
            4_096,
            2_048,
            512,
            512,
            policy,
            false,
            BTreeMap::new(),
        )
        .unwrap()
    }

    fn work_limit() -> RaccWorkV1 {
        RaccWorkV1 {
            logical_input_tokens: 1_000,
            uncached_input_tokens: 900,
            cached_input_tokens: 100,
            reasoning_tokens: 1_000,
            visible_output_tokens: 1_000,
            tool_calls: 100,
            verifier_work: 1_000,
            fallback_work: 10_000,
            latency_micros: 10_000_000,
            peak_memory_bytes: 64 * 1024 * 1024,
        }
    }

    fn envelope() -> WorkerEnvelope {
        WorkerEnvelope {
            fuel: 10_000,
            deadline_ms: 10_000,
            io_bytes: 1_000_000,
            output_bytes: 1_000_000,
            memory_bytes: 64 * 1024 * 1024,
            processes: 4,
            risk_units: 10,
            worker_steps: 1_000,
        }
    }

    fn reserve() -> FallbackReserveV1 {
        FallbackReserveV1 {
            deoptimization_envelope: envelope(),
            raw_baseline_envelope: envelope(),
            deoptimization_work_limit: work_limit(),
            raw_baseline_work_limit: work_limit(),
        }
    }

    fn usage() -> RouteUsageV1 {
        RouteUsageV1 {
            fuel: 10,
            elapsed_ms: 2,
            io_bytes: 100,
            output_bytes: 10,
            memory_bytes: 1_024,
            processes: 1,
            risk_units: 1,
            worker_steps: 2,
            work: RaccWorkV1 {
                logical_input_tokens: 10,
                uncached_input_tokens: 8,
                cached_input_tokens: 2,
                reasoning_tokens: 0,
                visible_output_tokens: 0,
                tool_calls: 1,
                verifier_work: 1,
                fallback_work: 100,
                latency_micros: 2_000,
                peak_memory_bytes: 1_024,
            },
        }
    }

    fn reasoning_safepoint(
        contract: &ReasoningContractV1,
        status: ReasoningStateStatusV1,
        state: DigestV1,
    ) -> ReasoningSafepointV1 {
        ReasoningSafepointV1::new(
            *d(1).as_bytes(),
            *d(2).as_bytes(),
            *d(3).as_bytes(),
            *contract.identity_digest().unwrap().as_bytes(),
            *contract.model_identity().as_bytes(),
            *state.as_bytes(),
            status,
            *d(10).as_bytes(),
            *d(11).as_bytes(),
            *d(12).as_bytes(),
            *d(13).as_bytes(),
            *d(14).as_bytes(),
        )
        .unwrap()
    }

    fn safepoint_claim(external_inventory: DigestV1) -> BaselineSafepointClaimV1 {
        let contract = reasoning_contract(NativeStatePolicyV1::ExactRequired);
        BaselineSafepointClaimV1::new(
            d(1),
            d(20),
            external_inventory,
            d(21),
            d(22),
            d(23),
            d(24),
            d(25),
            d(26),
            d(27),
            contract.clone(),
            reasoning_safepoint(&contract, ReasoningStateStatusV1::ExactPreserved, d(28)),
            BaselineReasoningEntryV1::ExactNativeContinuation {
                opaque_state_digest: d(28),
                parent_response_digest: d(29),
                session_identity_digest: d(30),
            },
            d(31),
            verifier_route(),
            reserve(),
            d(32),
            d(33),
            d(14),
        )
        .unwrap()
    }

    struct TestResolver {
        bytes: Vec<u8>,
    }

    impl Resolver for TestResolver {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (object_id.0 == sha256(&self.bytes)).then_some(self.bytes.as_slice())
        }
        fn trusted_operator_version<'a>(&'a self, operator_id: &str) -> Option<&'a str> {
            (operator_id == "deopt-verifier").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, parser_id: &str) -> Option<&'a str> {
            (parser_id == "deopt-parser").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, index_id: &str) -> Option<&'a str> {
            (index_id == "deopt-index").then_some("1")
        }
    }

    fn certificate(bytes: Vec<u8>) -> (EvidenceCertificate<'static>, TestResolver) {
        let digest = sha256(&bytes);
        let span = SpanRef {
            object_id: ObjectId(digest),
            object_digest: digest,
            byte_start: 0,
            byte_len: bytes.len() as u64,
            span_digest: digest,
        };
        (
            EvidenceCertificate {
                query: Query::TestTrace { test: TestId(9) },
                spans: vec![span],
                payload: Cow::Owned(bytes.clone()),
                provenance: Provenance {
                    parser_id: "deopt-parser".into(),
                    parser_version: "1".into(),
                    index_id: "deopt-index".into(),
                    index_version: "1".into(),
                    operator_id: "deopt-verifier".into(),
                    operator_version: "1".into(),
                },
                completeness: CompletenessWitness::TestTrace {
                    operator: OperatorLock {
                        operator_id: "deopt-verifier".into(),
                        operator_version: "1".into(),
                    },
                    test: TestId(9),
                    exit_code: 0,
                    trace_digest: digest,
                },
                input_token_cost: 0,
                backend_work_units: 1,
            },
            TestResolver { bytes },
        )
    }

    fn capture(external_inventory: DigestV1) -> BaselineSafepointEvidenceV1 {
        let claim = safepoint_claim(external_inventory);
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        BaselineSafepointEvidenceV1::verify_owner_scoped(claim, &evidence).unwrap()
    }

    fn effect_program(snapshot: DigestV1) -> EffectProgramV1 {
        EffectProgramV1::new(
            snapshot,
            "deoptimization_test",
            vec![EffectTargetV1 {
                owner: ArtifactOwnerV1::FsZero,
                target_digest: d(40),
                required_snapshot: snapshot,
            }],
            vec![],
            vec![TypedEffectOperationV1::ReplaceExactFile {
                target: d(40),
                expected_before: d(41),
                replacement: d(42),
            }],
            vec![],
            EffectVerificationPlanV1::new(vec![EffectVerificationStepV1 {
                verifier_digest: d(43),
                predicate_digest: d(44),
                environment_digest: d(45),
                required_snapshot: snapshot,
                verifier_class: CwirVerifierClassV1::ExactChecker,
            }])
            .unwrap(),
            EffectRollbackV1::Journaled,
        )
        .unwrap()
    }

    fn resource(
        kind: TransactionResourceKindV1,
        scope: u8,
        baseline: DigestV1,
        access: TransactionAccessV1,
    ) -> TransactionResourceRequirementV1 {
        TransactionResourceRequirementV1 {
            owner: if kind == TransactionResourceKindV1::ProjectFilesystem {
                ArtifactOwnerV1::FsZero
            } else {
                ArtifactOwnerV1::ZeroStack
            },
            kind,
            scope_digest: d(scope),
            baseline_state_digest: baseline,
            access,
            authority_digest: d(scope.wrapping_add(1)),
        }
    }

    struct AbortedFixture {
        receipt: TransactionReceiptV1,
        action_digest: DigestV1,
        candidate_state: DigestV1,
        closure_manifest_digest: DigestV1,
        external_inventory_digest: DigestV1,
    }

    fn aborted_fixture(external_debt: bool) -> AbortedFixture {
        let snapshot = d(1);
        let candidate = d(60);
        let program = effect_program(snapshot);
        let project = resource(
            TransactionResourceKindV1::ProjectFilesystem,
            50,
            snapshot,
            TransactionAccessV1::ReadWrite,
        );
        let external = resource(
            if external_debt {
                TransactionResourceKindV1::ExternalDatabase
            } else {
                TransactionResourceKindV1::Time
            },
            52,
            d(53),
            if external_debt {
                TransactionAccessV1::ReadWrite
            } else {
                TransactionAccessV1::Read
            },
        );
        let request = EffectClosureRequestV1::new(&program, vec![project, external]).unwrap();
        let manifest = EffectClosureManifestV1::new(
            &request,
            vec![
                EffectResourceClosureV1 {
                    requirement: project,
                    isolation: ResourceIsolationModeV1::Journaled,
                    restoration: ResourceRestorationModeV1::JournalRollback,
                },
                EffectResourceClosureV1 {
                    requirement: external,
                    isolation: if external_debt {
                        ResourceIsolationModeV1::Transactional
                    } else {
                        ResourceIsolationModeV1::RecordedReplay
                    },
                    restoration: if external_debt {
                        ResourceRestorationModeV1::TransactionRollback
                    } else {
                        ResourceRestorationModeV1::RecordedReplay
                    },
                },
            ],
        )
        .unwrap();
        let boundary = validate_effect_closure_v1(&request, &manifest).unwrap();
        let temp = tempdir().unwrap();
        let paths = JournalPathsV1::new(
            temp.path().join("root.json"),
            temp.path().join("journal.json"),
            temp.path().join("cartridge.json"),
            temp.path().join("owner-death.json"),
            temp.path().join("recovery.json"),
        )
        .unwrap();
        initialize_published_root_v1(&paths, snapshot).unwrap();
        let binding = effect_journal_binding_v1(
            &boundary,
            d(61),
            DurableProfileIdV1::PortableStrict,
            candidate,
            d(62),
        )
        .unwrap();
        let receipt = begin_effect_transaction_v1(paths, binding, &boundary)
            .unwrap()
            .abort()
            .unwrap();
        AbortedFixture {
            receipt,
            action_digest: boundary.action_digest(),
            candidate_state: candidate,
            closure_manifest_digest: boundary.manifest_digest(),
            external_inventory_digest: boundary.external_inventory_digest(),
        }
    }

    fn plan(fixture: &AbortedFixture) -> DeoptimizationPlanV1 {
        let plan = DeoptimizationPlanV1::for_fail_closed(
            capture(fixture.external_inventory_digest),
            FailureCode::PerformanceUnknown,
            d(70),
            fixture.action_digest,
            fixture.candidate_state,
            fixture.closure_manifest_digest,
            d(71),
            d(76),
            d(77),
        )
        .unwrap();
        let claim = plan.claim();
        let keys = serde_json::to_value(&claim)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected = [
            "schema_version",
            "safepoint_certificate_digest",
            "trigger",
            "candidate_action_digest",
            "candidate_state_digest",
            "candidate_closure_manifest_digest",
            "prior_work_receipt_digest",
            "kernel_binding_digest",
            "kernel_admission_digest",
            "plan_digest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(keys, expected);
        let bytes = claim.canonical_bytes().unwrap();
        assert_eq!(
            DeoptimizationPlanClaimV1::from_canonical_bytes(&bytes).unwrap(),
            claim
        );
        plan
    }

    #[test]
    fn deoptimization_contract_digest_is_stable() {
        // c5f93c285cf17cc7eca78ae3eec89d12cfda52667bbafdec9ae0b2ba20fdc69c
        assert_eq!(
            deoptimization_contract_digest_v1(),
            DigestV1::from_bytes([
                0xc5, 0xf9, 0x3c, 0x28, 0x5c, 0xf1, 0x7c, 0xc7, 0xec, 0xa7, 0x8a, 0xe3, 0xee, 0xc8,
                0x9d, 0x12, 0xcf, 0xda, 0x52, 0x66, 0x7b, 0xba, 0xfd, 0xec, 0x9a, 0xe0, 0xb2, 0xba,
                0x20, 0xfd, 0xc6, 0x9c,
            ])
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../conformance/schemas/exact-deoptimization-resume-v1.schema.json"
            )))
            .to_hex(),
            DEOPTIMIZATION_RESUME_SCHEMA_SHA256_V1
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../conformance/schemas/exact-deoptimization-execution-v1.schema.json"
            )))
            .to_hex(),
            DEOPTIMIZATION_EXECUTION_SCHEMA_SHA256_V1
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../conformance/schemas/exact-deoptimization-plan-v1.schema.json"
            )))
            .to_hex(),
            DEOPTIMIZATION_PLAN_SCHEMA_SHA256_V1
        );
    }

    #[test]
    fn exact_restoration_mints_linear_baseline_invocation_and_g8_closure() {
        let fixture = aborted_fixture(false);
        let plan = plan(&fixture);
        let claim = BaselineRestorationClaimV1::new(
            &plan,
            fixture.receipt.receipt_digest(),
            d(72),
            d(73),
            d(74),
            verifier_route(),
            usage(),
        )
        .unwrap();
        let restoration_keys = serde_json::to_value(&claim)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected_restoration_keys = [
            "schema_version",
            "plan_digest",
            "safepoint_certificate_digest",
            "transaction_receipt_digest",
            "restored_project_root",
            "restored_external_inventory_digest",
            "restored_reasoning_contract_digest",
            "restored_fixed_model_digest",
            "restored_reasoning_entry_digest",
            "raw_baseline_identity_digest",
            "raw_baseline_input_digest",
            "raw_decision_view_digest",
            "candidate_overlay_disposition_digest",
            "visible_buffer_disposition_digest",
            "prior_receipt_head_digest",
            "successor_receipt_head_digest",
            "restoration_verifier_identity_digest",
            "deoptimization_usage",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(restoration_keys, expected_restoration_keys);
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        let permit =
            BaselineResumePermitV1::verify_restoration(plan, fixture.receipt, claim, &evidence)
                .unwrap();
        permit.validate().unwrap();
        let resume_record = permit.record();
        let bytes = resume_record.canonical_bytes().unwrap();
        assert_eq!(
            BaselineResumeReceiptRecordV1::from_canonical_bytes(&bytes).unwrap(),
            resume_record
        );
        let invocation = permit.into_invocation().unwrap();
        invocation.validate().unwrap();
        assert_eq!(invocation.raw_baseline_identity_digest(), d(22));
        assert_eq!(invocation.project_snapshot_root(), d(1));
        let mut overrun = usage();
        overrun.fuel = envelope().fuel + 1;
        assert_eq!(
            BaselineExecutionClaimV1::new(
                &invocation,
                d(80),
                d(81),
                d(83),
                d(84),
                d(85),
                d(86),
                overrun,
                d(82),
                verifier_route(),
            )
            .unwrap_err()
            .failure_code(),
            DeoptimizationFailureCodeV1::ResourceReserveExceeded
        );
        let execution_claim = BaselineExecutionClaimV1::new(
            &invocation,
            d(80),
            d(81),
            d(83),
            d(84),
            d(85),
            d(86),
            usage(),
            d(82),
            verifier_route(),
        )
        .unwrap();
        let keys = serde_json::to_value(&execution_claim)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected = [
            "schema_version",
            "invocation_digest",
            "resume_permit_digest",
            "transaction_receipt_digest",
            "project_snapshot_root",
            "raw_baseline_identity_digest",
            "raw_baseline_input_digest",
            "raw_decision_view_digest",
            "baseline_reasoning_contract_digest",
            "reasoning_entry_digest",
            "predecessor_receipt_head_digest",
            "output_digest",
            "effects_digest",
            "baseline_action_digest",
            "baseline_acceptance_digest",
            "baseline_successor_root",
            "baseline_transaction_receipt_digest",
            "raw_baseline_usage",
            "successor_receipt_head_digest",
            "execution_verifier_identity_digest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(keys, expected);
        let claim_bytes = execution_claim.canonical_bytes().unwrap();
        assert_eq!(
            BaselineExecutionClaimV1::from_canonical_bytes(&claim_bytes).unwrap(),
            execution_claim
        );
        let (certificate, resolver) = self::certificate(claim_bytes);
        let evidence = verify(&certificate, &resolver).unwrap();
        let execution_receipt =
            BaselineExecutionReceiptV1::verify_execution(invocation, execution_claim, &evidence)
                .unwrap();
        let execution_record = execution_receipt.record();
        let bytes = execution_record.canonical_bytes().unwrap();
        assert_eq!(
            BaselineExecutionReceiptRecordV1::from_canonical_bytes(&bytes).unwrap(),
            execution_record
        );
        let mut tampered = execution_record.clone();
        tampered.receipt_digest = d(99);
        assert_eq!(
            tampered.validate().unwrap_err().failure_code(),
            DeoptimizationFailureCodeV1::CertificateDigestMismatch
        );
        let closure = TransactionClosure::from_baseline_execution(execution_receipt).unwrap();
        assert_eq!(closure.kind(), ClosureKind::Fallback);
        assert_eq!(closure.root(), *d(85).as_bytes());
        assert_eq!(closure.transaction_receipt_digest(), *d(86).as_bytes());
    }

    #[test]
    fn bare_fallback_transaction_cannot_enter_g8_without_resume_authority() {
        let fixture = aborted_fixture(false);
        let error = TransactionClosure::from_receipt(fixture.receipt).unwrap_err();
        assert_eq!(error.code, FailureCode::UnaccountedFallback);
    }

    #[test]
    fn journal_root_only_recovery_never_mints_exact_deoptimization() {
        let fixture = aborted_fixture(true);
        assert_eq!(
            fixture.receipt.restoration_scope(),
            RestorationScopeV1::ProjectJournalRootOnly
        );
        let plan = plan(&fixture);
        let claim = BaselineRestorationClaimV1::new(
            &plan,
            fixture.receipt.receipt_digest(),
            d(72),
            d(73),
            d(74),
            verifier_route(),
            usage(),
        )
        .unwrap();
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        assert_eq!(
            BaselineResumePermitV1::verify_restoration(plan, fixture.receipt, claim, &evidence,)
                .unwrap_err()
                .failure_code(),
            DeoptimizationFailureCodeV1::TransactionMismatch
        );
    }

    #[test]
    fn stale_reasoning_entry_and_resource_overrun_fail_closed() {
        let contract = reasoning_contract(NativeStatePolicyV1::ExactRequired);
        let bad = BaselineSafepointClaimV1::new(
            d(1),
            d(20),
            d(21),
            d(22),
            d(23),
            d(24),
            d(25),
            d(26),
            d(27),
            d(28),
            contract.clone(),
            reasoning_safepoint(&contract, ReasoningStateStatusV1::ExactCleanRestart, d(29)),
            BaselineReasoningEntryV1::ExactNativeContinuation {
                opaque_state_digest: d(29),
                parent_response_digest: d(30),
                session_identity_digest: d(31),
            },
            d(32),
            verifier_route(),
            reserve(),
            d(33),
            d(34),
            d(14),
        );
        assert_eq!(
            bad.unwrap_err().failure_code(),
            DeoptimizationFailureCodeV1::ReasoningEntryMismatch
        );

        let fixture = aborted_fixture(false);
        let plan = plan(&fixture);
        let mut overrun = usage();
        overrun.fuel = envelope().fuel + 1;
        let claim = BaselineRestorationClaimV1::new(
            &plan,
            fixture.receipt.receipt_digest(),
            d(72),
            d(73),
            d(74),
            verifier_route(),
            overrun,
        )
        .unwrap();
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        assert_eq!(
            BaselineResumePermitV1::verify_restoration(plan, fixture.receipt, claim, &evidence,)
                .unwrap_err()
                .failure_code(),
            DeoptimizationFailureCodeV1::ResourceReserveExceeded
        );
    }

    #[test]
    fn clean_start_is_exact_only_when_frozen_as_the_baseline_entry() {
        let contract = reasoning_contract(NativeStatePolicyV1::CleanRestart);
        let claim = BaselineSafepointClaimV1::new(
            d(1),
            d(20),
            d(21),
            d(22),
            d(23),
            d(24),
            d(25),
            d(26),
            d(27),
            d(28),
            contract.clone(),
            reasoning_safepoint(&contract, ReasoningStateStatusV1::ExactCleanRestart, d(29)),
            BaselineReasoningEntryV1::CanonicalCleanStart {
                clean_start_identity_digest: d(29),
            },
            d(30),
            verifier_route(),
            reserve(),
            d(31),
            d(32),
            d(14),
        )
        .unwrap();
        claim.validate().unwrap();
        let mut bytes = claim.canonical_bytes().unwrap();
        bytes.push(b'\n');
        assert_eq!(
            BaselineSafepointClaimV1::from_canonical_bytes(&bytes)
                .unwrap_err()
                .failure_code(),
            DeoptimizationFailureCodeV1::NonCanonicalEncoding
        );
    }
}
