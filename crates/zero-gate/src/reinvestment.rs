//! Proof-carrying causal-slack reinvestment.
//!
//! A portfolio is admitted only against frozen native resource coordinates,
//! keeps the raw fallback reserve, and preserves the fixed-model reasoning
//! contract. Every branch remains isolated, measured, and quality-gated. Only
//! exact verifier evidence can select a strictly improved dominant branch;
//! these records do not directly authorize publication.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zero_abi::{
    DigestV1, StrictReasoningAdmissionV1, canonical_json, reasoning_contract_digest_v1,
};
use zero_cert::VerifiedEvidence;
use zero_ledger::{CausalWorkReceiptV1, ParentCounterIdentityV1, causal_work_contract_digest_v1};

use crate::{
    q99::{q99_verifier_identity_v1, verified_evidence_digest, verify_exact_successful_payload},
    quality::{QualityAdmissionV1, QualitySelectionV1, quality_envelope_contract_digest_v1},
    transaction::{
        RestorationScopeV1, TransactionDispositionV1, TransactionReceiptV1,
        transaction_contract_digest_v1,
    },
};

pub const REINVESTMENT_CONTRACT_VERSION_V1: u16 = 1;
pub const REINVESTMENT_PLAN_SCHEMA_VERSION_V1: &str = "zerostack.reinvestment.plan.v1";
pub const REINVESTMENT_SELECTION_SCHEMA_VERSION_V1: &str = "zerostack.reinvestment.selection.v1";
pub const REINVESTMENT_MAX_ACTIONS_V1: usize = 128;
pub const REINVESTMENT_MAX_COORDINATES_V1: usize = 32;
pub const REINVESTMENT_MAX_CANONICAL_BYTES_V1: usize = 1_048_576;
pub const REINVESTMENT_MAX_ID_BYTES_V1: usize = 256;

pub const REINVESTMENT_PLAN_SCHEMA_SHA256_V1: &str =
    "37f696fd177940c8852300d8448d5d6aabe6d4ddf827256dc0f12871af5c6671";
pub const REINVESTMENT_SELECTION_SCHEMA_SHA256_V1: &str =
    "24c80a2156361fbd5f8da599823476fe08d3bc0f8d28cee4de96c8536358ed3f";

const RESOURCE_IDENTITY_DOMAIN_V1: &[u8] = b"zerostack.reinvestment.resource_identity.v1\0";
const ACTION_CLAIM_DOMAIN_V1: &[u8] = b"zerostack.reinvestment.action_claim.v1\0";
const ACTION_AUTHORITY_DOMAIN_V1: &[u8] = b"zerostack.reinvestment.action_authority.v1\0";
const PLAN_DOMAIN_V1: &[u8] = b"zerostack.reinvestment.plan.v1\0";
const BRANCH_DOMAIN_V1: &[u8] = b"zerostack.reinvestment.branch.v1\0";
const SELECTION_CLAIM_DOMAIN_V1: &[u8] = b"zerostack.reinvestment.selection_claim.v1\0";
const SELECTION_AUTHORITY_DOMAIN_V1: &[u8] = b"zerostack.reinvestment.selection_authority.v1\0";
const BASELINE_DOMAIN_V1: &[u8] = b"zerostack.reinvestment.baseline.v1\0";
const CONTRACT_DOMAIN_V1: &[u8] = b"zerostack.reinvestment.contract.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReinvestmentActionKindV1 {
    ExactEvidenceExpansion,
    SameModelSecondCandidate,
    SameModelCritique,
    HigherReasoningEffort,
    StrongerVerifier,
    CounterexampleSearch,
    AdditionalTests,
    MutationTesting,
    DifferentialExecution,
    FormalProofAttempt,
    LargerFallbackReserve,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReinvestmentCostPositionV1 {
    WithinRawBaseline,
    DeclaredAdditionalBudget,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortfolioSelectionBasisV1 {
    CertifiedMaximum,
    PairwiseDominant,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReinvestmentBaselineReasonV1 {
    NoStrictlyImprovedBranch,
    QualityRejected,
    ExecutionFailed,
    DominanceUnresolved,
    OperatorSelectedBaseline,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReinvestmentTransactionDispositionV1 {
    CandidateCommitted,
    BaselineRootRecovered,
}

impl From<TransactionDispositionV1> for ReinvestmentTransactionDispositionV1 {
    fn from(value: TransactionDispositionV1) -> Self {
        match value {
            TransactionDispositionV1::CandidateCommitted => Self::CandidateCommitted,
            TransactionDispositionV1::BaselineRootRecovered => Self::BaselineRootRecovered,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResourceAmountV1 {
    pub identity: ParentCounterIdentityV1,
    pub amount: u64,
}

impl NativeResourceAmountV1 {
    fn identity_digest(&self) -> Result<DigestV1, ReinvestmentErrorV1> {
        resource_identity_digest(&self.identity)
    }

    fn validate(&self) -> Result<(), ReinvestmentErrorV1> {
        validate_counter_identity(&self.identity)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResourceVectorV1 {
    pub coordinates: Vec<NativeResourceAmountV1>,
}

impl NativeResourceVectorV1 {
    pub fn new(coordinates: Vec<NativeResourceAmountV1>) -> Result<Self, ReinvestmentErrorV1> {
        if coordinates.is_empty() || coordinates.len() > REINVESTMENT_MAX_COORDINATES_V1 {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::InvalidResourceVector,
                "native resource vector must contain 1..=32 coordinates",
            ));
        }
        let mut keyed = coordinates
            .into_iter()
            .map(|coordinate| Ok((coordinate.identity_digest()?, coordinate)))
            .collect::<Result<Vec<_>, ReinvestmentErrorV1>>()?;
        keyed.sort_by_key(|(digest, _)| *digest);
        let vector = Self {
            coordinates: keyed
                .into_iter()
                .map(|(_, coordinate)| coordinate)
                .collect(),
        };
        vector.validate()?;
        Ok(vector)
    }

    pub fn zeros_like(&self) -> Self {
        Self {
            coordinates: self
                .coordinates
                .iter()
                .map(|coordinate| NativeResourceAmountV1 {
                    identity: coordinate.identity.clone(),
                    amount: 0,
                })
                .collect(),
        }
    }

    pub fn validate(&self) -> Result<(), ReinvestmentErrorV1> {
        if self.coordinates.is_empty() || self.coordinates.len() > REINVESTMENT_MAX_COORDINATES_V1 {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::InvalidResourceVector,
                "native resource vector must contain 1..=32 coordinates",
            ));
        }
        let mut prior = None;
        for coordinate in &self.coordinates {
            coordinate.validate()?;
            let digest = coordinate.identity_digest()?;
            if prior.is_some_and(|previous| previous >= digest) {
                return Err(reinvestment_error(
                    ReinvestmentFailureCodeV1::InvalidResourceVector,
                    "resource coordinates must be uniquely sorted by frozen counter identity",
                ));
            }
            prior = Some(digest);
        }
        Ok(())
    }

    pub fn same_coordinates(&self, other: &Self) -> Result<bool, ReinvestmentErrorV1> {
        self.validate()?;
        other.validate()?;
        Ok(self.coordinates.len() == other.coordinates.len()
            && self
                .coordinates
                .iter()
                .zip(&other.coordinates)
                .all(|(left, right)| left.identity == right.identity))
    }

    pub fn checked_add(&self, other: &Self) -> Result<Self, ReinvestmentErrorV1> {
        require_same_coordinates(self, other)?;
        let coordinates = self
            .coordinates
            .iter()
            .zip(&other.coordinates)
            .map(|(left, right)| {
                Ok(NativeResourceAmountV1 {
                    identity: left.identity.clone(),
                    amount: left.amount.checked_add(right.amount).ok_or_else(|| {
                        reinvestment_error(
                            ReinvestmentFailureCodeV1::ArithmeticOverflow,
                            "native resource addition overflowed",
                        )
                    })?,
                })
            })
            .collect::<Result<Vec<_>, ReinvestmentErrorV1>>()?;
        Ok(Self { coordinates })
    }

    pub fn checked_sub(&self, other: &Self) -> Result<Self, ReinvestmentErrorV1> {
        require_same_coordinates(self, other)?;
        let coordinates = self
            .coordinates
            .iter()
            .zip(&other.coordinates)
            .map(|(left, right)| {
                Ok(NativeResourceAmountV1 {
                    identity: left.identity.clone(),
                    amount: left.amount.checked_sub(right.amount).ok_or_else(|| {
                        reinvestment_error(
                            ReinvestmentFailureCodeV1::BudgetExceeded,
                            "native resource subtraction would be negative",
                        )
                    })?,
                })
            })
            .collect::<Result<Vec<_>, ReinvestmentErrorV1>>()?;
        Ok(Self { coordinates })
    }

    pub fn componentwise_le(&self, other: &Self) -> Result<bool, ReinvestmentErrorV1> {
        require_same_coordinates(self, other)?;
        Ok(self
            .coordinates
            .iter()
            .zip(&other.coordinates)
            .all(|(left, right)| left.amount <= right.amount))
    }

    pub fn any_nonzero(&self) -> bool {
        self.coordinates
            .iter()
            .any(|coordinate| coordinate.amount != 0)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReinvestmentActionClaimV1 {
    pub schema_version: String,
    pub scope_digest: DigestV1,
    pub comparison_identity_digest: DigestV1,
    pub raw_baseline_identity_digest: DigestV1,
    pub assembly_manifest_digest: DigestV1,
    pub baseline_state_digest: DigestV1,
    pub action_digest: DigestV1,
    pub action_kind: ReinvestmentActionKindV1,
    pub candidate_identity_digest: DigestV1,
    pub transaction_action_digest: DigestV1,
    pub isolation_scope_digest: DigestV1,
    pub baseline_reasoning_contract_digest: DigestV1,
    pub candidate_reasoning_contract_digest: DigestV1,
    pub reasoning_admission_digest: DigestV1,
    pub reserved_cost: NativeResourceVectorV1,
    pub isolation_verifier_identity_digest: DigestV1,
}

impl ReinvestmentActionClaimV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scope_digest: DigestV1,
        comparison_identity_digest: DigestV1,
        raw_baseline_identity_digest: DigestV1,
        assembly_manifest_digest: DigestV1,
        baseline_state_digest: DigestV1,
        action_digest: DigestV1,
        action_kind: ReinvestmentActionKindV1,
        candidate_identity_digest: DigestV1,
        transaction_action_digest: DigestV1,
        isolation_scope_digest: DigestV1,
        reasoning_admission: &StrictReasoningAdmissionV1,
        reserved_cost: NativeResourceVectorV1,
        isolation_verifier_identity_digest: DigestV1,
    ) -> Result<Self, ReinvestmentErrorV1> {
        reasoning_admission.validate().map_err(reasoning_error)?;
        let claim = Self {
            schema_version: REINVESTMENT_PLAN_SCHEMA_VERSION_V1.into(),
            scope_digest,
            comparison_identity_digest,
            raw_baseline_identity_digest,
            assembly_manifest_digest,
            baseline_state_digest,
            action_digest,
            action_kind,
            candidate_identity_digest,
            transaction_action_digest,
            isolation_scope_digest,
            baseline_reasoning_contract_digest: reasoning_admission.baseline_contract_digest(),
            candidate_reasoning_contract_digest: reasoning_admission.candidate_contract_digest(),
            reasoning_admission_digest: reasoning_admission.digest(),
            reserved_cost,
            isolation_verifier_identity_digest,
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), ReinvestmentErrorV1> {
        if self.schema_version != REINVESTMENT_PLAN_SCHEMA_VERSION_V1 {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::SchemaVersionMismatch,
                "reinvestment action claim schema version mismatch",
            ));
        }
        require_nonzero(
            "reinvestment action claim",
            &[
                self.scope_digest,
                self.comparison_identity_digest,
                self.raw_baseline_identity_digest,
                self.assembly_manifest_digest,
                self.baseline_state_digest,
                self.action_digest,
                self.candidate_identity_digest,
                self.transaction_action_digest,
                self.isolation_scope_digest,
                self.baseline_reasoning_contract_digest,
                self.candidate_reasoning_contract_digest,
                self.reasoning_admission_digest,
                self.isolation_verifier_identity_digest,
            ],
        )?;
        if self.action_kind == ReinvestmentActionKindV1::HigherReasoningEffort {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::UnsupportedReasoningChange,
                "higher reasoning effort needs an explicit ordered cross-class theorem; string labels cannot authorize it",
            ));
        }
        self.reserved_cost.validate()?;
        if !self.reserved_cost.any_nonzero() {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::InvalidResourceVector,
                "a reinvestment action must reserve nonzero work",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReinvestmentErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<DigestV1, ReinvestmentErrorV1> {
        self.validate()?;
        digest_serializable(ACTION_CLAIM_DOMAIN_V1, self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedReinvestmentActionRecordV1 {
    pub claim: ReinvestmentActionClaimV1,
    pub claim_digest: DigestV1,
    pub evidence_digest: DigestV1,
    pub authority_digest: DigestV1,
}

impl VerifiedReinvestmentActionRecordV1 {
    pub fn validate(&self) -> Result<(), ReinvestmentErrorV1> {
        self.claim.validate()?;
        if self.claim_digest != self.claim.digest()? {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::DigestMismatch,
                "reinvestment action claim digest mismatch",
            ));
        }
        require_nonzero(
            "verified reinvestment action",
            &[self.evidence_digest, self.authority_digest],
        )?;
        if self.authority_digest != action_authority_digest(self)? {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::DigestMismatch,
                "reinvestment action authority digest mismatch",
            ));
        }
        Ok(())
    }
}

/// Opaque proof that a declared action has exact isolation evidence.
pub struct VerifiedReinvestmentActionV1 {
    record: VerifiedReinvestmentActionRecordV1,
}

impl VerifiedReinvestmentActionV1 {
    pub fn record(&self) -> &VerifiedReinvestmentActionRecordV1 {
        &self.record
    }

    pub const fn action_digest(&self) -> DigestV1 {
        self.record.claim.action_digest
    }
}

pub fn verify_reinvestment_action_v1(
    claim: ReinvestmentActionClaimV1,
    reasoning_admission: &StrictReasoningAdmissionV1,
    isolation_evidence: &VerifiedEvidence<'_, '_>,
) -> Result<VerifiedReinvestmentActionV1, ReinvestmentErrorV1> {
    claim.validate()?;
    reasoning_admission.validate().map_err(reasoning_error)?;
    if claim.baseline_reasoning_contract_digest != reasoning_admission.baseline_contract_digest()
        || claim.candidate_reasoning_contract_digest
            != reasoning_admission.candidate_contract_digest()
        || claim.reasoning_admission_digest != reasoning_admission.digest()
        || !reasoning_admission.same_comparison_class()
    {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::ActionBindingMismatch,
            "action reasoning fields do not bind the opaque fixed-model admission",
        ));
    }
    let claim_bytes = claim.canonical_bytes()?;
    verify_exact_successful_payload(&claim_bytes, isolation_evidence).map_err(q99_error)?;
    if q99_verifier_identity_v1(isolation_evidence) != claim.isolation_verifier_identity_digest {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::VerifierIdentityMismatch,
            "isolation evidence verifier identity does not match the action claim",
        ));
    }
    let mut record = VerifiedReinvestmentActionRecordV1 {
        claim_digest: claim.digest()?,
        evidence_digest: verified_evidence_digest(isolation_evidence).map_err(q99_error)?,
        claim,
        authority_digest: DigestV1::ZERO,
    };
    record.authority_digest = action_authority_digest(&record)?;
    record.validate()?;
    Ok(VerifiedReinvestmentActionV1 { record })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReinvestmentPlanRecordV1 {
    pub contract_version: u16,
    pub scope_digest: DigestV1,
    pub comparison_identity_digest: DigestV1,
    pub raw_baseline_identity_digest: DigestV1,
    pub raw_baseline_receipt_digest: DigestV1,
    pub assembly_manifest_digest: DigestV1,
    pub baseline_state_digest: DigestV1,
    pub baseline_reasoning_contract_digest: DigestV1,
    pub baseline_budget: NativeResourceVectorV1,
    pub declared_additional_budget: NativeResourceVectorV1,
    pub strict_candidate_guarded_bound: NativeResourceVectorV1,
    pub fallback_reserve: NativeResourceVectorV1,
    pub causal_slack: NativeResourceVectorV1,
    pub actions: Vec<VerifiedReinvestmentActionRecordV1>,
    pub cost_position: ReinvestmentCostPositionV1,
    pub plan_digest: DigestV1,
}

impl ReinvestmentPlanRecordV1 {
    pub fn validate(&self) -> Result<(), ReinvestmentErrorV1> {
        if self.contract_version != REINVESTMENT_CONTRACT_VERSION_V1 {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::SchemaVersionMismatch,
                "reinvestment plan contract version mismatch",
            ));
        }
        require_nonzero(
            "reinvestment plan",
            &[
                self.scope_digest,
                self.comparison_identity_digest,
                self.raw_baseline_identity_digest,
                self.raw_baseline_receipt_digest,
                self.assembly_manifest_digest,
                self.baseline_state_digest,
                self.baseline_reasoning_contract_digest,
                self.plan_digest,
            ],
        )?;
        validate_plan_vectors(self)?;
        validate_action_records(self)?;
        let (causal_slack, cost_position) = compute_plan_accounting(self)?;
        if self.causal_slack != causal_slack || self.cost_position != cost_position {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::BudgetMismatch,
                "stored causal slack or cost position differs from checked coordinate arithmetic",
            ));
        }
        if self.plan_digest != plan_digest(self)? {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::DigestMismatch,
                "reinvestment plan digest mismatch",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReinvestmentErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ReinvestmentErrorV1> {
        if bytes.len() > REINVESTMENT_MAX_CANONICAL_BYTES_V1 {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::CanonicalPayloadTooLarge,
                "reinvestment plan exceeds the canonical byte bound",
            ));
        }
        let record: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        record.validate()?;
        if record.canonical_bytes()? != bytes {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::NonCanonicalEncoding,
                "reinvestment plan is not canonical sorted-key JSON",
            ));
        }
        Ok(record)
    }
}

/// Opaque resource admission for one isolated portfolio. It cannot publish.
pub struct ReinvestmentPlanAuthorityV1 {
    record: ReinvestmentPlanRecordV1,
}

impl ReinvestmentPlanAuthorityV1 {
    pub fn record(&self) -> &ReinvestmentPlanRecordV1 {
        &self.record
    }

    pub const fn digest(&self) -> DigestV1 {
        self.record.plan_digest
    }

    pub const fn permits_publication(&self) -> bool {
        false
    }
}

#[allow(clippy::too_many_arguments)]
pub fn admit_reinvestment_plan_v1(
    scope_digest: DigestV1,
    comparison_identity_digest: DigestV1,
    raw_baseline_identity_digest: DigestV1,
    raw_baseline_receipt_digest: DigestV1,
    assembly_manifest_digest: DigestV1,
    baseline_state_digest: DigestV1,
    baseline_reasoning_contract_digest: DigestV1,
    baseline_budget: NativeResourceVectorV1,
    declared_additional_budget: NativeResourceVectorV1,
    strict_candidate_guarded_bound: NativeResourceVectorV1,
    fallback_reserve: NativeResourceVectorV1,
    actions: Vec<VerifiedReinvestmentActionV1>,
) -> Result<ReinvestmentPlanAuthorityV1, ReinvestmentErrorV1> {
    if actions.is_empty() || actions.len() > REINVESTMENT_MAX_ACTIONS_V1 {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::InvalidPortfolio,
            "reinvestment portfolio must contain 1..=128 actions",
        ));
    }
    let mut action_records = actions
        .into_iter()
        .map(|action| action.record)
        .collect::<Vec<_>>();
    action_records.sort_by_key(|record| record.claim.action_digest);
    let mut record = ReinvestmentPlanRecordV1 {
        contract_version: REINVESTMENT_CONTRACT_VERSION_V1,
        scope_digest,
        comparison_identity_digest,
        raw_baseline_identity_digest,
        raw_baseline_receipt_digest,
        assembly_manifest_digest,
        baseline_state_digest,
        baseline_reasoning_contract_digest,
        baseline_budget,
        declared_additional_budget,
        strict_candidate_guarded_bound,
        fallback_reserve,
        causal_slack: NativeResourceVectorV1 {
            coordinates: Vec::new(),
        },
        actions: action_records,
        cost_position: ReinvestmentCostPositionV1::WithinRawBaseline,
        plan_digest: DigestV1::ZERO,
    };
    validate_plan_vectors(&record)?;
    validate_action_records(&record)?;
    let (causal_slack, cost_position) = compute_plan_accounting(&record)?;
    record.causal_slack = causal_slack;
    record.cost_position = cost_position;
    record.plan_digest = plan_digest(&record)?;
    record.validate()?;
    Ok(ReinvestmentPlanAuthorityV1 { record })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReinvestmentBranchRecordV1 {
    pub contract_version: u16,
    pub plan_digest: DigestV1,
    pub action_digest: DigestV1,
    pub transaction_receipt_digest: DigestV1,
    pub transaction_disposition: ReinvestmentTransactionDispositionV1,
    pub candidate_state_digest: DigestV1,
    pub measured_work_receipt_digests: Vec<DigestV1>,
    pub measured_work: NativeResourceVectorV1,
    pub quality_admission_digest: DigestV1,
    pub quality_selection: QualitySelectionV1,
    pub strict_protected_improvement: bool,
    pub branch_digest: DigestV1,
}

impl ReinvestmentBranchRecordV1 {
    pub fn validate(&self) -> Result<(), ReinvestmentErrorV1> {
        if self.contract_version != REINVESTMENT_CONTRACT_VERSION_V1 {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::SchemaVersionMismatch,
                "reinvestment branch contract version mismatch",
            ));
        }
        require_nonzero(
            "reinvestment branch",
            &[
                self.plan_digest,
                self.action_digest,
                self.transaction_receipt_digest,
                self.candidate_state_digest,
                self.quality_admission_digest,
                self.branch_digest,
            ],
        )?;
        if self.measured_work_receipt_digests.is_empty()
            || self.measured_work_receipt_digests.len() != self.measured_work.coordinates.len()
            || self.measured_work_receipt_digests.contains(&DigestV1::ZERO)
        {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::IncompleteMeasuredWork,
                "every native coordinate needs one nonzero causal-work receipt digest",
            ));
        }
        self.measured_work.validate()?;
        if self.quality_selection == QualitySelectionV1::Candidate
            && self.transaction_disposition
                != ReinvestmentTransactionDispositionV1::CandidateCommitted
        {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::TransactionMismatch,
                "a quality-selected branch needs an isolated committed transaction",
            ));
        }
        if self.branch_digest != branch_digest(self)? {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::DigestMismatch,
                "reinvestment branch digest mismatch",
            ));
        }
        Ok(())
    }
}

/// Opaque branch result built from a real transaction, measured work, and G7.
pub struct VerifiedReinvestmentBranchV1 {
    record: ReinvestmentBranchRecordV1,
}

impl VerifiedReinvestmentBranchV1 {
    pub fn record(&self) -> &ReinvestmentBranchRecordV1 {
        &self.record
    }

    pub fn is_strictly_improved_candidate(&self) -> bool {
        self.record.quality_selection == QualitySelectionV1::Candidate
            && self.record.strict_protected_improvement
            && matches!(
                self.record.transaction_disposition,
                ReinvestmentTransactionDispositionV1::CandidateCommitted
            )
    }
}

pub fn complete_reinvestment_branch_v1(
    plan: &ReinvestmentPlanAuthorityV1,
    action_digest: DigestV1,
    transaction: &TransactionReceiptV1,
    measured_work: &[CausalWorkReceiptV1],
    quality_admission: &QualityAdmissionV1,
) -> Result<VerifiedReinvestmentBranchV1, ReinvestmentErrorV1> {
    plan.record.validate()?;
    let action = plan
        .record
        .actions
        .iter()
        .find(|record| record.claim.action_digest == action_digest)
        .ok_or_else(|| {
            reinvestment_error(
                ReinvestmentFailureCodeV1::UnknownAction,
                "branch action is not in the admitted portfolio",
            )
        })?;
    transaction.canonical_bytes().map_err(transaction_error)?;
    quality_admission.validate().map_err(quality_error)?;
    if transaction.action_digest() != action.claim.transaction_action_digest
        || transaction.baseline_state() != plan.record.baseline_state_digest
        || transaction.external_restoration_debt_count() != 0
        || (transaction.disposition() == TransactionDispositionV1::BaselineRootRecovered
            && transaction.restoration_scope() != RestorationScopeV1::DeclaredEffectClosure)
    {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::TransactionMismatch,
            "branch transaction, baseline, or complete restoration does not match the isolated action",
        ));
    }
    if quality_admission.scope_digest() != plan.record.scope_digest
        || quality_admission.comparison_identity_digest() != plan.record.comparison_identity_digest
        || quality_admission.raw_baseline_identity_digest()
            != plan.record.raw_baseline_identity_digest
        || quality_admission.baseline_receipt_digest() != plan.record.raw_baseline_receipt_digest
        || quality_admission.candidate_identity_digest()
            != Some(action.claim.candidate_identity_digest)
    {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::QualityBindingMismatch,
            "branch quality admission binds another task, comparison, baseline, or candidate",
        ));
    }
    if quality_admission.selection() == QualitySelectionV1::Candidate
        && transaction.disposition() != TransactionDispositionV1::CandidateCommitted
    {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::TransactionMismatch,
            "quality selected a candidate that the isolated transaction did not commit",
        ));
    }
    let (measured_vector, receipt_digests) = measured_work_vector(
        measured_work,
        plan.record.assembly_manifest_digest,
        &action.claim.reserved_cost,
    )?;
    let mut record = ReinvestmentBranchRecordV1 {
        contract_version: REINVESTMENT_CONTRACT_VERSION_V1,
        plan_digest: plan.digest(),
        action_digest,
        transaction_receipt_digest: transaction.receipt_digest(),
        transaction_disposition: transaction.disposition().into(),
        candidate_state_digest: transaction.candidate_state(),
        measured_work_receipt_digests: receipt_digests,
        measured_work: measured_vector,
        quality_admission_digest: quality_admission.digest(),
        quality_selection: quality_admission.selection(),
        strict_protected_improvement: quality_admission.strict_improvement(),
        branch_digest: DigestV1::ZERO,
    };
    record.branch_digest = branch_digest(&record)?;
    record.validate()?;
    Ok(VerifiedReinvestmentBranchV1 { record })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReinvestmentSelectionClaimV1 {
    pub schema_version: String,
    pub plan_digest: DigestV1,
    pub selected_action_digest: DigestV1,
    pub selected_branch_digest: DigestV1,
    pub selected_quality_admission_digest: DigestV1,
    pub branch_digests: Vec<DigestV1>,
    pub selection_basis: PortfolioSelectionBasisV1,
    pub dominance_relation_digest: DigestV1,
    pub verifier_identity_digest: DigestV1,
}

impl ReinvestmentSelectionClaimV1 {
    pub fn validate(&self) -> Result<(), ReinvestmentErrorV1> {
        if self.schema_version != REINVESTMENT_SELECTION_SCHEMA_VERSION_V1 {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::SchemaVersionMismatch,
                "reinvestment selection claim schema version mismatch",
            ));
        }
        require_nonzero(
            "reinvestment selection claim",
            &[
                self.plan_digest,
                self.selected_action_digest,
                self.selected_branch_digest,
                self.selected_quality_admission_digest,
                self.dominance_relation_digest,
                self.verifier_identity_digest,
            ],
        )?;
        if self.branch_digests.is_empty()
            || self.branch_digests.len() > REINVESTMENT_MAX_ACTIONS_V1
            || self.branch_digests.contains(&DigestV1::ZERO)
            || !strictly_sorted_unique(&self.branch_digests)
            || self
                .branch_digests
                .binary_search(&self.selected_branch_digest)
                .is_err()
        {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::InvalidPortfolio,
                "selection branch digests must be nonempty, unique, and sorted",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReinvestmentErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<DigestV1, ReinvestmentErrorV1> {
        self.validate()?;
        digest_serializable(SELECTION_CLAIM_DOMAIN_V1, self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReinvestmentSelectionRecordV1 {
    pub contract_version: u16,
    pub claim: ReinvestmentSelectionClaimV1,
    pub claim_digest: DigestV1,
    pub evidence_digest: DigestV1,
    pub authority_digest: DigestV1,
}

impl ReinvestmentSelectionRecordV1 {
    pub fn validate(&self) -> Result<(), ReinvestmentErrorV1> {
        if self.contract_version != REINVESTMENT_CONTRACT_VERSION_V1 {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::SchemaVersionMismatch,
                "reinvestment selection contract version mismatch",
            ));
        }
        self.claim.validate()?;
        if self.claim_digest != self.claim.digest()? {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::DigestMismatch,
                "reinvestment selection claim digest mismatch",
            ));
        }
        require_nonzero(
            "reinvestment selection",
            &[self.evidence_digest, self.authority_digest],
        )?;
        if self.authority_digest != selection_authority_digest(self)? {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::DigestMismatch,
                "reinvestment selection authority digest mismatch",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReinvestmentErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ReinvestmentErrorV1> {
        if bytes.len() > REINVESTMENT_MAX_CANONICAL_BYTES_V1 {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::CanonicalPayloadTooLarge,
                "reinvestment selection exceeds the canonical byte bound",
            ));
        }
        let record: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        record.validate()?;
        if record.canonical_bytes()? != bytes {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::NonCanonicalEncoding,
                "reinvestment selection is not canonical sorted-key JSON",
            ));
        }
        Ok(record)
    }
}

/// Opaque portfolio decision. A later G8/G9 gate must still authorize publish.
pub struct ReinvestmentSelectionAuthorityV1 {
    record: ReinvestmentSelectionRecordV1,
}

impl ReinvestmentSelectionAuthorityV1 {
    pub fn record(&self) -> &ReinvestmentSelectionRecordV1 {
        &self.record
    }

    pub const fn selected_action_digest(&self) -> DigestV1 {
        self.record.claim.selected_action_digest
    }

    pub const fn selected_branch_digest(&self) -> DigestV1 {
        self.record.claim.selected_branch_digest
    }

    pub const fn selected_quality_admission_digest(&self) -> DigestV1 {
        self.record.claim.selected_quality_admission_digest
    }

    pub const fn permits_publication(&self) -> bool {
        false
    }
}

pub fn reinvestment_selection_claim_v1(
    plan: &ReinvestmentPlanAuthorityV1,
    branches: &[&VerifiedReinvestmentBranchV1],
    selected_action_digest: DigestV1,
    selection_basis: PortfolioSelectionBasisV1,
    dominance_relation_digest: DigestV1,
    verifier_identity_digest: DigestV1,
) -> Result<ReinvestmentSelectionClaimV1, ReinvestmentErrorV1> {
    let mut branch_records = complete_branch_set_ref(plan, branches)?;
    let selected = branch_records
        .iter()
        .find(|branch| branch.action_digest == selected_action_digest)
        .ok_or_else(|| {
            reinvestment_error(
                ReinvestmentFailureCodeV1::UnknownAction,
                "selected action has no completed branch",
            )
        })?;
    if selected.quality_selection != QualitySelectionV1::Candidate
        || !selected.strict_protected_improvement
        || selected.transaction_disposition
            != ReinvestmentTransactionDispositionV1::CandidateCommitted
    {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::NoProtectedMarginalGain,
            "selected reinvestment branch lacks strict protected quality gain",
        ));
    }
    let selected_branch_digest = selected.branch_digest;
    let selected_quality_admission_digest = selected.quality_admission_digest;
    branch_records.sort_by_key(|branch| branch.branch_digest);
    let claim = ReinvestmentSelectionClaimV1 {
        schema_version: REINVESTMENT_SELECTION_SCHEMA_VERSION_V1.into(),
        plan_digest: plan.digest(),
        selected_action_digest,
        selected_branch_digest,
        selected_quality_admission_digest,
        branch_digests: branch_records
            .iter()
            .map(|branch| branch.branch_digest)
            .collect(),
        selection_basis,
        dominance_relation_digest,
        verifier_identity_digest,
    };
    claim.validate()?;
    Ok(claim)
}

pub fn select_reinvestment_winner_v1(
    plan: &ReinvestmentPlanAuthorityV1,
    branches: Vec<VerifiedReinvestmentBranchV1>,
    claim: ReinvestmentSelectionClaimV1,
    dominance_evidence: &VerifiedEvidence<'_, '_>,
) -> Result<ReinvestmentSelectionAuthorityV1, ReinvestmentErrorV1> {
    let branch_refs = branches.iter().collect::<Vec<_>>();
    let expected = reinvestment_selection_claim_v1(
        plan,
        &branch_refs,
        claim.selected_action_digest,
        claim.selection_basis,
        claim.dominance_relation_digest,
        claim.verifier_identity_digest,
    )?;
    if claim != expected {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::ActionBindingMismatch,
            "selection claim does not bind the complete admitted branch set",
        ));
    }
    verify_exact_successful_payload(&claim.canonical_bytes()?, dominance_evidence)
        .map_err(q99_error)?;
    if q99_verifier_identity_v1(dominance_evidence) != claim.verifier_identity_digest {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::VerifierIdentityMismatch,
            "dominance verifier identity does not match the selection claim",
        ));
    }
    let mut record = ReinvestmentSelectionRecordV1 {
        contract_version: REINVESTMENT_CONTRACT_VERSION_V1,
        claim_digest: claim.digest()?,
        evidence_digest: verified_evidence_digest(dominance_evidence).map_err(q99_error)?,
        claim,
        authority_digest: DigestV1::ZERO,
    };
    record.authority_digest = selection_authority_digest(&record)?;
    record.validate()?;
    Ok(ReinvestmentSelectionAuthorityV1 { record })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReinvestmentBaselineRecordV1 {
    pub contract_version: u16,
    pub plan_digest: DigestV1,
    pub branch_digests: Vec<DigestV1>,
    pub reason: ReinvestmentBaselineReasonV1,
    pub baseline_digest: DigestV1,
}

impl ReinvestmentBaselineRecordV1 {
    pub fn validate(&self) -> Result<(), ReinvestmentErrorV1> {
        if self.contract_version != REINVESTMENT_CONTRACT_VERSION_V1
            || self.plan_digest == DigestV1::ZERO
            || self.branch_digests.is_empty()
            || self.branch_digests.contains(&DigestV1::ZERO)
            || !strictly_sorted_unique(&self.branch_digests)
        {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::InvalidPortfolio,
                "baseline reinvestment record is incomplete or unsorted",
            ));
        }
        if self.baseline_digest != baseline_record_digest(self)? {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::DigestMismatch,
                "reinvestment baseline record digest mismatch",
            ));
        }
        Ok(())
    }
}

pub fn fall_back_reinvestment_portfolio_v1(
    plan: &ReinvestmentPlanAuthorityV1,
    branches: Vec<VerifiedReinvestmentBranchV1>,
    reason: ReinvestmentBaselineReasonV1,
) -> Result<ReinvestmentBaselineRecordV1, ReinvestmentErrorV1> {
    let mut records = complete_branch_set(plan, branches)?;
    records.sort_by_key(|record| record.branch_digest);
    let mut baseline = ReinvestmentBaselineRecordV1 {
        contract_version: REINVESTMENT_CONTRACT_VERSION_V1,
        plan_digest: plan.digest(),
        branch_digests: records.iter().map(|record| record.branch_digest).collect(),
        reason,
        baseline_digest: DigestV1::ZERO,
    };
    baseline.baseline_digest = baseline_record_digest(&baseline)?;
    baseline.validate()?;
    Ok(baseline)
}

pub fn reinvestment_contract_manifest_v1() -> Value {
    json!({
        "action_isolation": "successful_exact_verified_evidence_over_canonical_action_claim",
        "arithmetic": "checked_u64_per_frozen_native_coordinate",
        "branch_requirements": [
            "isolated_transaction_receipt", "complete_measured_causal_work",
            "quality_readmission_against_frozen_raw_baseline"
        ],
        "canonical_encoding": "sorted_key_json_no_whitespace",
        "contract_version": REINVESTMENT_CONTRACT_VERSION_V1,
        "cost_positions": ["within_raw_baseline", "declared_additional_budget"],
        "fallback": "reserved_before_portfolio_and_selected_on_unresolved_dominance",
        "higher_effort": "unsupported_without_explicit_ordered_cross_class_theorem",
        "linked_contracts": {
            "causal_work": causal_work_contract_digest_v1(),
            "quality": quality_envelope_contract_digest_v1(),
            "reasoning": reasoning_contract_digest_v1(),
            "transaction": transaction_contract_digest_v1(),
        },
        "negative_space": [
            "predicted_gain_as_measured_gain",
            "more_compute_as_monotone_quality",
            "distributional_evidence_as_individual_selection",
            "unmeasured_work_as_zero",
            "cross_coordinate_resource_substitution",
            "reinvestment_record_as_direct_publication_authority",
        ],
        "published_plan_schema_sha256": REINVESTMENT_PLAN_SCHEMA_SHA256_V1,
        "published_selection_schema_sha256": REINVESTMENT_SELECTION_SCHEMA_SHA256_V1,
        "selection": "exact_verified_maximum_or_pairwise_dominance_and_strict_protected_gain",
    })
}

pub fn reinvestment_contract_digest_v1() -> DigestV1 {
    digest_value(CONTRACT_DOMAIN_V1, &reinvestment_contract_manifest_v1())
}

fn validate_plan_vectors(record: &ReinvestmentPlanRecordV1) -> Result<(), ReinvestmentErrorV1> {
    record.baseline_budget.validate()?;
    require_same_coordinates(&record.baseline_budget, &record.declared_additional_budget)?;
    require_same_coordinates(
        &record.baseline_budget,
        &record.strict_candidate_guarded_bound,
    )?;
    require_same_coordinates(&record.baseline_budget, &record.fallback_reserve)?;
    if !record.fallback_reserve.any_nonzero() {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::MissingFallbackReserve,
            "amplify mode must retain a nonzero raw-baseline fallback reserve",
        ));
    }
    Ok(())
}

fn validate_action_records(record: &ReinvestmentPlanRecordV1) -> Result<(), ReinvestmentErrorV1> {
    if record.actions.is_empty() || record.actions.len() > REINVESTMENT_MAX_ACTIONS_V1 {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::InvalidPortfolio,
            "reinvestment portfolio must contain 1..=128 actions",
        ));
    }
    let mut prior = None;
    for action in &record.actions {
        action.validate()?;
        let claim = &action.claim;
        if prior.is_some_and(|previous| previous >= claim.action_digest) {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::DuplicateAction,
                "portfolio actions must be uniquely sorted",
            ));
        }
        prior = Some(claim.action_digest);
        if claim.scope_digest != record.scope_digest
            || claim.comparison_identity_digest != record.comparison_identity_digest
            || claim.raw_baseline_identity_digest != record.raw_baseline_identity_digest
            || claim.assembly_manifest_digest != record.assembly_manifest_digest
            || claim.baseline_state_digest != record.baseline_state_digest
            || claim.baseline_reasoning_contract_digest != record.baseline_reasoning_contract_digest
        {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::ActionBindingMismatch,
                "portfolio action binds another scope, comparison, baseline, assembly, state, or reasoning contract",
            ));
        }
        require_same_coordinates(&record.baseline_budget, &claim.reserved_cost)?;
    }
    Ok(())
}

fn compute_plan_accounting(
    record: &ReinvestmentPlanRecordV1,
) -> Result<(NativeResourceVectorV1, ReinvestmentCostPositionV1), ReinvestmentErrorV1> {
    let base_committed = record
        .strict_candidate_guarded_bound
        .checked_add(&record.fallback_reserve)?;
    let causal_slack = record.baseline_budget.checked_sub(&base_committed)?;
    let committed = record
        .actions
        .iter()
        .try_fold(base_committed, |total, action| {
            total.checked_add(&action.claim.reserved_cost)
        })?;
    if committed.componentwise_le(&record.baseline_budget)? {
        return Ok((causal_slack, ReinvestmentCostPositionV1::WithinRawBaseline));
    }
    let expanded = record
        .baseline_budget
        .checked_add(&record.declared_additional_budget)?;
    if committed.componentwise_le(&expanded)? {
        return Ok((
            causal_slack,
            ReinvestmentCostPositionV1::DeclaredAdditionalBudget,
        ));
    }
    Err(reinvestment_error(
        ReinvestmentFailureCodeV1::BudgetExceeded,
        "candidate, fallback reserve, and reinvestment exceed baseline plus declared additional budget",
    ))
}

fn complete_branch_set(
    plan: &ReinvestmentPlanAuthorityV1,
    branches: Vec<VerifiedReinvestmentBranchV1>,
) -> Result<Vec<ReinvestmentBranchRecordV1>, ReinvestmentErrorV1> {
    plan.record.validate()?;
    if branches.len() != plan.record.actions.len() {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::IncompletePortfolio,
            "every admitted reinvestment action needs one completed branch",
        ));
    }
    let mut records = branches
        .into_iter()
        .map(|branch| branch.record)
        .collect::<Vec<_>>();
    records.sort_by_key(|record| record.action_digest);
    for (action, branch) in plan.record.actions.iter().zip(&records) {
        branch.validate()?;
        if branch.plan_digest != plan.digest() || branch.action_digest != action.claim.action_digest
        {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::IncompletePortfolio,
                "branch set is duplicated, incomplete, or bound to another plan",
            ));
        }
    }
    Ok(records)
}

fn complete_branch_set_ref(
    plan: &ReinvestmentPlanAuthorityV1,
    branches: &[&VerifiedReinvestmentBranchV1],
) -> Result<Vec<ReinvestmentBranchRecordV1>, ReinvestmentErrorV1> {
    plan.record.validate()?;
    if branches.len() != plan.record.actions.len() {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::IncompletePortfolio,
            "every admitted reinvestment action needs one completed branch",
        ));
    }
    let mut records = branches
        .iter()
        .map(|branch| branch.record.clone())
        .collect::<Vec<_>>();
    records.sort_by_key(|record| record.action_digest);
    for (action, branch) in plan.record.actions.iter().zip(&records) {
        branch.validate()?;
        if branch.plan_digest != plan.digest() || branch.action_digest != action.claim.action_digest
        {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::IncompletePortfolio,
                "branch set is duplicated, incomplete, or bound to another plan",
            ));
        }
    }
    Ok(records)
}

fn measured_work_vector(
    receipts: &[CausalWorkReceiptV1],
    assembly_manifest_digest: DigestV1,
    reserved: &NativeResourceVectorV1,
) -> Result<(NativeResourceVectorV1, Vec<DigestV1>), ReinvestmentErrorV1> {
    if receipts.len() != reserved.coordinates.len() {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::IncompleteMeasuredWork,
            "every reserved native coordinate needs one causal-work receipt",
        ));
    }
    let mut entries = Vec::with_capacity(receipts.len());
    for receipt in receipts {
        receipt.validate().map_err(causal_work_error)?;
        if receipt.assembly_manifest_digest != assembly_manifest_digest {
            return Err(reinvestment_error(
                ReinvestmentFailureCodeV1::WorkBindingMismatch,
                "causal-work receipt binds another assembly manifest",
            ));
        }
        entries.push((
            resource_identity_digest(&receipt.measurement.identity)?,
            NativeResourceAmountV1 {
                identity: receipt.measurement.identity.clone(),
                amount: receipt.observed_total,
            },
            receipt.receipt_digest,
        ));
    }
    entries.sort_by_key(|entry| entry.0);
    let measured =
        NativeResourceVectorV1::new(entries.iter().map(|entry| entry.1.clone()).collect())?;
    if !measured.same_coordinates(reserved)? || !measured.componentwise_le(reserved)? {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::WorkBoundExceeded,
            "measured reinvestment work exceeds or differs from its reserved native coordinates",
        ));
    }
    Ok((measured, entries.into_iter().map(|entry| entry.2).collect()))
}

fn resource_identity_digest(
    identity: &ParentCounterIdentityV1,
) -> Result<DigestV1, ReinvestmentErrorV1> {
    validate_counter_identity(identity)?;
    digest_serializable(RESOURCE_IDENTITY_DOMAIN_V1, identity)
}

fn validate_counter_identity(
    identity: &ParentCounterIdentityV1,
) -> Result<(), ReinvestmentErrorV1> {
    if identity.counter_id.is_empty()
        || identity.counter_id.len() > REINVESTMENT_MAX_ID_BYTES_V1
        || identity.boundary_digest == DigestV1::ZERO
        || identity.adapter_digest == DigestV1::ZERO
        || identity.platform_profile_digest == DigestV1::ZERO
    {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::InvalidResourceVector,
            "native resource counter identity is empty, oversized, or incomplete",
        ));
    }
    Ok(())
}

fn require_same_coordinates(
    left: &NativeResourceVectorV1,
    right: &NativeResourceVectorV1,
) -> Result<(), ReinvestmentErrorV1> {
    if !left.same_coordinates(right)? {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::ResourceCoordinateMismatch,
            "native resource vectors use different frozen counter coordinates",
        ));
    }
    Ok(())
}

fn action_authority_digest(
    record: &VerifiedReinvestmentActionRecordV1,
) -> Result<DigestV1, ReinvestmentErrorV1> {
    Ok(digest_value(
        ACTION_AUTHORITY_DOMAIN_V1,
        &json!({
            "claim_digest": record.claim_digest,
            "evidence_digest": record.evidence_digest,
        }),
    ))
}

fn plan_digest(record: &ReinvestmentPlanRecordV1) -> Result<DigestV1, ReinvestmentErrorV1> {
    Ok(digest_value(
        PLAN_DOMAIN_V1,
        &json!({
            "actions": record.actions,
            "assembly_manifest_digest": record.assembly_manifest_digest,
            "baseline_budget": record.baseline_budget,
            "baseline_reasoning_contract_digest": record.baseline_reasoning_contract_digest,
            "baseline_state_digest": record.baseline_state_digest,
            "causal_slack": record.causal_slack,
            "comparison_identity_digest": record.comparison_identity_digest,
            "contract_version": record.contract_version,
            "cost_position": record.cost_position,
            "declared_additional_budget": record.declared_additional_budget,
            "fallback_reserve": record.fallback_reserve,
            "raw_baseline_identity_digest": record.raw_baseline_identity_digest,
            "raw_baseline_receipt_digest": record.raw_baseline_receipt_digest,
            "scope_digest": record.scope_digest,
            "strict_candidate_guarded_bound": record.strict_candidate_guarded_bound,
        }),
    ))
}

fn branch_digest(record: &ReinvestmentBranchRecordV1) -> Result<DigestV1, ReinvestmentErrorV1> {
    Ok(digest_value(
        BRANCH_DOMAIN_V1,
        &json!({
            "action_digest": record.action_digest,
            "candidate_state_digest": record.candidate_state_digest,
            "contract_version": record.contract_version,
            "measured_work": record.measured_work,
            "measured_work_receipt_digests": record.measured_work_receipt_digests,
            "plan_digest": record.plan_digest,
            "quality_admission_digest": record.quality_admission_digest,
            "quality_selection": record.quality_selection,
            "strict_protected_improvement": record.strict_protected_improvement,
            "transaction_disposition": record.transaction_disposition,
            "transaction_receipt_digest": record.transaction_receipt_digest,
        }),
    ))
}

fn selection_authority_digest(
    record: &ReinvestmentSelectionRecordV1,
) -> Result<DigestV1, ReinvestmentErrorV1> {
    Ok(digest_value(
        SELECTION_AUTHORITY_DOMAIN_V1,
        &json!({
            "claim_digest": record.claim_digest,
            "contract_version": record.contract_version,
            "evidence_digest": record.evidence_digest,
        }),
    ))
}

fn baseline_record_digest(
    record: &ReinvestmentBaselineRecordV1,
) -> Result<DigestV1, ReinvestmentErrorV1> {
    Ok(digest_value(
        BASELINE_DOMAIN_V1,
        &json!({
            "branch_digests": record.branch_digests,
            "contract_version": record.contract_version,
            "plan_digest": record.plan_digest,
            "reason": record.reason,
        }),
    ))
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ReinvestmentErrorV1> {
    let value = serde_json::to_value(value).map_err(json_error)?;
    Ok(canonical_json(&value).into_bytes())
}

fn digest_serializable<T: Serialize>(
    domain: &[u8],
    value: &T,
) -> Result<DigestV1, ReinvestmentErrorV1> {
    Ok(domain_digest(domain, &canonical_bytes(value)?))
}

fn digest_value(domain: &[u8], value: &Value) -> DigestV1 {
    domain_digest(domain, canonical_json(value).as_bytes())
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> DigestV1 {
    let mut input = Vec::with_capacity(domain.len() + bytes.len());
    input.extend_from_slice(domain);
    input.extend_from_slice(bytes);
    DigestV1::from_bytes(zero_abi::sha256(&input))
}

fn require_nonzero(label: &'static str, values: &[DigestV1]) -> Result<(), ReinvestmentErrorV1> {
    if values.contains(&DigestV1::ZERO) {
        return Err(reinvestment_error(
            ReinvestmentFailureCodeV1::ZeroDigest,
            format!("{label} contains a zero digest"),
        ));
    }
    Ok(())
}

fn strictly_sorted_unique(values: &[DigestV1]) -> bool {
    values.windows(2).all(|window| window[0] < window[1])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReinvestmentFailureCodeV1 {
    SchemaVersionMismatch,
    ZeroDigest,
    InvalidResourceVector,
    ResourceCoordinateMismatch,
    ArithmeticOverflow,
    BudgetExceeded,
    BudgetMismatch,
    MissingFallbackReserve,
    InvalidPortfolio,
    DuplicateAction,
    ActionBindingMismatch,
    UnknownAction,
    UnsupportedReasoningChange,
    EvidencePayloadMismatch,
    VerifierIdentityMismatch,
    TransactionMismatch,
    QualityBindingMismatch,
    IncompleteMeasuredWork,
    WorkBindingMismatch,
    WorkBoundExceeded,
    IncompletePortfolio,
    NoProtectedMarginalGain,
    DigestMismatch,
    CanonicalPayloadTooLarge,
    NonCanonicalEncoding,
    InvalidJson,
    DependencyFailure,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ReinvestmentErrorV1 {
    code: ReinvestmentFailureCodeV1,
    detail: String,
}

impl ReinvestmentErrorV1 {
    pub const fn failure_code(&self) -> ReinvestmentFailureCodeV1 {
        self.code
    }
}

impl fmt::Display for ReinvestmentErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "reinvestment {:?}: {}", self.code, self.detail)
    }
}

impl Error for ReinvestmentErrorV1 {}

fn reinvestment_error(
    code: ReinvestmentFailureCodeV1,
    detail: impl Into<String>,
) -> ReinvestmentErrorV1 {
    ReinvestmentErrorV1 {
        code,
        detail: detail.into(),
    }
}

fn json_error(error: serde_json::Error) -> ReinvestmentErrorV1 {
    reinvestment_error(ReinvestmentFailureCodeV1::InvalidJson, error.to_string())
}

fn q99_error(error: crate::q99::Q99ErrorV1) -> ReinvestmentErrorV1 {
    reinvestment_error(
        ReinvestmentFailureCodeV1::EvidencePayloadMismatch,
        error.to_string(),
    )
}

fn reasoning_error(error: zero_abi::ReasoningContractErrorV1) -> ReinvestmentErrorV1 {
    reinvestment_error(
        ReinvestmentFailureCodeV1::UnsupportedReasoningChange,
        error.to_string(),
    )
}

fn transaction_error(error: crate::transaction::TransactionErrorV1) -> ReinvestmentErrorV1 {
    reinvestment_error(
        ReinvestmentFailureCodeV1::DependencyFailure,
        error.to_string(),
    )
}

fn quality_error(error: crate::quality::QualityEnvelopeErrorV1) -> ReinvestmentErrorV1 {
    reinvestment_error(
        ReinvestmentFailureCodeV1::DependencyFailure,
        error.to_string(),
    )
}

fn causal_work_error(error: zero_ledger::CausalWorkErrorV1) -> ReinvestmentErrorV1 {
    reinvestment_error(
        ReinvestmentFailureCodeV1::DependencyFailure,
        error.to_string(),
    )
}

