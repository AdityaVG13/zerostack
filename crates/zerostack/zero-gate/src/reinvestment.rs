//! Proof-carrying causal-slack reinvestment. A portfolio is admitted only against frozen native
//! resource coordinates, keeps the raw fallback reserve, and preserves the fixed-model reasoning
//! contract. Every branch remains isolated, measured, and quality-gated.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zero_abi::{Sha256Digest, StrictReasoningAdmission, canonical_json, reasoning_contract_digest};
use zero_cert::VerifiedEvidence;
use zero_ledger::{CausalWorkReceipt, ParentCounterIdentity, causal_work_contract_digest};

use crate::{
    q99::{q99_verifier_identity, verified_evidence_digest, verify_exact_successful_payload},
    quality::{QualityAdmission, QualitySelection, quality_envelope_contract_digest},
    transaction::{
        RestorationScope, TransactionDisposition, TransactionReceipt, transaction_contract_digest,
    },
};

pub const REINVESTMENT_CONTRACT_VERSION: u16 = 1;
pub const REINVESTMENT_PLAN_SCHEMA_VERSION: &str = "zerostack.reinvestment.plan";
pub const REINVESTMENT_SELECTION_SCHEMA_VERSION: &str = "zerostack.reinvestment.selection";
pub const REINVESTMENT_MAX_ACTIONS: usize = 128;
pub const REINVESTMENT_MAX_COORDINATES: usize = 32;
pub const REINVESTMENT_MAX_CANONICAL_BYTES: usize = 1_048_576;
pub const REINVESTMENT_MAX_ID_BYTES: usize = 256;

pub const REINVESTMENT_PLAN_SCHEMA_SHA256: &str =
    "37f696fd177940c8852300d8448d5d6aabe6d4ddf827256dc0f12871af5c6671";
pub const REINVESTMENT_SELECTION_SCHEMA_SHA256: &str =
    "24c80a2156361fbd5f8da599823476fe08d3bc0f8d28cee4de96c8536358ed3f";

const RESOURCE_IDENTITY_DOMAIN: &[u8] = b"zerostack.reinvestment.resource_identity\0";
const ACTION_CLAIM_DOMAIN: &[u8] = b"zerostack.reinvestment.action_claim\0";
const ACTION_AUTHORITY_DOMAIN: &[u8] = b"zerostack.reinvestment.action_authority\0";
const PLAN_DOMAIN: &[u8] = b"zerostack.reinvestment.plan\0";
const BRANCH_DOMAIN: &[u8] = b"zerostack.reinvestment.branch\0";
const SELECTION_CLAIM_DOMAIN: &[u8] = b"zerostack.reinvestment.selection_claim\0";
const SELECTION_AUTHORITY_DOMAIN: &[u8] = b"zerostack.reinvestment.selection_authority\0";
const BASELINE_DOMAIN: &[u8] = b"zerostack.reinvestment.baseline\0";
const CONTRACT_DOMAIN: &[u8] = b"zerostack.reinvestment.contract\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReinvestmentActionKind {
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
pub enum ReinvestmentCostPosition {
    WithinRawBaseline,
    DeclaredAdditionalBudget,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortfolioSelectionBasis {
    CertifiedMaximum,
    PairwiseDominant,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReinvestmentBaselineReason {
    NoStrictlyImprovedBranch,
    QualityRejected,
    ExecutionFailed,
    DominanceUnresolved,
    OperatorSelectedBaseline,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReinvestmentTransactionDisposition {
    CandidateCommitted,
    BaselineRootRecovered,
}

impl From<TransactionDisposition> for ReinvestmentTransactionDisposition {
    fn from(value: TransactionDisposition) -> Self {
        match value {
            TransactionDisposition::CandidateCommitted => Self::CandidateCommitted,
            TransactionDisposition::BaselineRootRecovered => Self::BaselineRootRecovered,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResourceAmount {
    pub identity: ParentCounterIdentity,
    pub amount: u64,
}

impl NativeResourceAmount {
    fn identity_digest(&self) -> Result<Sha256Digest, ReinvestmentError> {
        resource_identity_digest(&self.identity)
    }

    fn validate(&self) -> Result<(), ReinvestmentError> {
        validate_counter_identity(&self.identity)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResourceVector {
    pub coordinates: Vec<NativeResourceAmount>,
}

impl NativeResourceVector {
    pub fn new(coordinates: Vec<NativeResourceAmount>) -> Result<Self, ReinvestmentError> {
        if coordinates.is_empty() || coordinates.len() > REINVESTMENT_MAX_COORDINATES {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::InvalidResourceVector,
                "native resource vector must contain 1..=32 coordinates",
            ));
        }
        let mut keyed = coordinates
            .into_iter()
            .map(|coordinate| Ok((coordinate.identity_digest()?, coordinate)))
            .collect::<Result<Vec<_>, ReinvestmentError>>()?;
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
                .map(|coordinate| NativeResourceAmount {
                    identity: coordinate.identity.clone(),
                    amount: 0,
                })
                .collect(),
        }
    }

    pub fn validate(&self) -> Result<(), ReinvestmentError> {
        if self.coordinates.is_empty() || self.coordinates.len() > REINVESTMENT_MAX_COORDINATES {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::InvalidResourceVector,
                "native resource vector must contain 1..=32 coordinates",
            ));
        }
        let mut prior = None;
        for coordinate in &self.coordinates {
            coordinate.validate()?;
            let digest = coordinate.identity_digest()?;
            if prior.is_some_and(|previous| previous >= digest) {
                return Err(reinvestment_error(
                    ReinvestmentFailureCode::InvalidResourceVector,
                    "resource coordinates must be uniquely sorted by frozen counter identity",
                ));
            }
            prior = Some(digest);
        }
        Ok(())
    }

    pub fn same_coordinates(&self, other: &Self) -> Result<bool, ReinvestmentError> {
        self.validate()?;
        other.validate()?;
        Ok(self.coordinates.len() == other.coordinates.len()
            && self
                .coordinates
                .iter()
                .zip(&other.coordinates)
                .all(|(left, right)| left.identity == right.identity))
    }

    pub fn checked_add(&self, other: &Self) -> Result<Self, ReinvestmentError> {
        require_same_coordinates(self, other)?;
        let coordinates = self
            .coordinates
            .iter()
            .zip(&other.coordinates)
            .map(|(left, right)| {
                Ok(NativeResourceAmount {
                    identity: left.identity.clone(),
                    amount: left.amount.checked_add(right.amount).ok_or_else(|| {
                        reinvestment_error(
                            ReinvestmentFailureCode::ArithmeticOverflow,
                            "native resource addition overflowed",
                        )
                    })?,
                })
            })
            .collect::<Result<Vec<_>, ReinvestmentError>>()?;
        Ok(Self { coordinates })
    }

    pub fn checked_sub(&self, other: &Self) -> Result<Self, ReinvestmentError> {
        require_same_coordinates(self, other)?;
        let coordinates = self
            .coordinates
            .iter()
            .zip(&other.coordinates)
            .map(|(left, right)| {
                Ok(NativeResourceAmount {
                    identity: left.identity.clone(),
                    amount: left.amount.checked_sub(right.amount).ok_or_else(|| {
                        reinvestment_error(
                            ReinvestmentFailureCode::BudgetExceeded,
                            "native resource subtraction would be negative",
                        )
                    })?,
                })
            })
            .collect::<Result<Vec<_>, ReinvestmentError>>()?;
        Ok(Self { coordinates })
    }

    pub fn componentwise_le(&self, other: &Self) -> Result<bool, ReinvestmentError> {
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
pub struct ReinvestmentActionClaim {
    pub schema_version: String,
    pub scope_digest: Sha256Digest,
    pub comparison_identity_digest: Sha256Digest,
    pub raw_baseline_identity_digest: Sha256Digest,
    pub assembly_manifest_digest: Sha256Digest,
    pub baseline_state_digest: Sha256Digest,
    pub action_digest: Sha256Digest,
    pub action_kind: ReinvestmentActionKind,
    pub candidate_identity_digest: Sha256Digest,
    pub transaction_action_digest: Sha256Digest,
    pub isolation_scope_digest: Sha256Digest,
    pub baseline_reasoning_contract_digest: Sha256Digest,
    pub candidate_reasoning_contract_digest: Sha256Digest,
    pub reasoning_admission_digest: Sha256Digest,
    pub reserved_cost: NativeResourceVector,
    pub isolation_verifier_identity_digest: Sha256Digest,
}

impl ReinvestmentActionClaim {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scope_digest: Sha256Digest,
        comparison_identity_digest: Sha256Digest,
        raw_baseline_identity_digest: Sha256Digest,
        assembly_manifest_digest: Sha256Digest,
        baseline_state_digest: Sha256Digest,
        action_digest: Sha256Digest,
        action_kind: ReinvestmentActionKind,
        candidate_identity_digest: Sha256Digest,
        transaction_action_digest: Sha256Digest,
        isolation_scope_digest: Sha256Digest,
        reasoning_admission: &StrictReasoningAdmission,
        reserved_cost: NativeResourceVector,
        isolation_verifier_identity_digest: Sha256Digest,
    ) -> Result<Self, ReinvestmentError> {
        reasoning_admission.validate().map_err(reasoning_error)?;
        let claim = Self {
            schema_version: REINVESTMENT_PLAN_SCHEMA_VERSION.into(),
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

    pub fn validate(&self) -> Result<(), ReinvestmentError> {
        if self.schema_version != REINVESTMENT_PLAN_SCHEMA_VERSION {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::SchemaVersionMismatch,
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
        if self.action_kind == ReinvestmentActionKind::HigherReasoningEffort {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::UnsupportedReasoningChange,
                "higher reasoning effort needs an explicit ordered cross-class theorem; string labels cannot authorize it",
            ));
        }
        self.reserved_cost.validate()?;
        if !self.reserved_cost.any_nonzero() {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::InvalidResourceVector,
                "a reinvestment action must reserve nonzero work",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReinvestmentError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<Sha256Digest, ReinvestmentError> {
        self.validate()?;
        digest_serializable(ACTION_CLAIM_DOMAIN, self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedReinvestmentActionRecord {
    pub claim: ReinvestmentActionClaim,
    pub claim_digest: Sha256Digest,
    pub evidence_digest: Sha256Digest,
    pub authority_digest: Sha256Digest,
}

impl VerifiedReinvestmentActionRecord {
    pub fn validate(&self) -> Result<(), ReinvestmentError> {
        self.claim.validate()?;
        if self.claim_digest != self.claim.digest()? {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::DigestMismatch,
                "reinvestment action claim digest mismatch",
            ));
        }
        require_nonzero(
            "verified reinvestment action",
            &[self.evidence_digest, self.authority_digest],
        )?;
        if self.authority_digest != action_authority_digest(self)? {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::DigestMismatch,
                "reinvestment action authority digest mismatch",
            ));
        }
        Ok(())
    }
}

/// Opaque proof that a declared action has exact isolation evidence.
pub struct VerifiedReinvestmentAction {
    record: VerifiedReinvestmentActionRecord,
}

impl VerifiedReinvestmentAction {
    pub fn record(&self) -> &VerifiedReinvestmentActionRecord {
        &self.record
    }

    pub const fn action_digest(&self) -> Sha256Digest {
        self.record.claim.action_digest
    }
}

pub fn verify_reinvestment_action(
    claim: ReinvestmentActionClaim,
    reasoning_admission: &StrictReasoningAdmission,
    isolation_evidence: &VerifiedEvidence<'_, '_>,
) -> Result<VerifiedReinvestmentAction, ReinvestmentError> {
    claim.validate()?;
    reasoning_admission.validate().map_err(reasoning_error)?;
    if claim.baseline_reasoning_contract_digest != reasoning_admission.baseline_contract_digest()
        || claim.candidate_reasoning_contract_digest
            != reasoning_admission.candidate_contract_digest()
        || claim.reasoning_admission_digest != reasoning_admission.digest()
        || !reasoning_admission.same_comparison_class()
    {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::ActionBindingMismatch,
            "action reasoning fields do not bind the opaque fixed-model admission",
        ));
    }
    let claim_bytes = claim.canonical_bytes()?;
    verify_exact_successful_payload(&claim_bytes, isolation_evidence).map_err(q99_error)?;
    if q99_verifier_identity(isolation_evidence) != claim.isolation_verifier_identity_digest {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::VerifierIdentityMismatch,
            "isolation evidence verifier identity does not match the action claim",
        ));
    }
    let mut record = VerifiedReinvestmentActionRecord {
        claim_digest: claim.digest()?,
        evidence_digest: verified_evidence_digest(isolation_evidence).map_err(q99_error)?,
        claim,
        authority_digest: Sha256Digest::ZERO,
    };
    record.authority_digest = action_authority_digest(&record)?;
    record.validate()?;
    Ok(VerifiedReinvestmentAction { record })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReinvestmentPlanRecord {
    pub contract_version: u16,
    pub scope_digest: Sha256Digest,
    pub comparison_identity_digest: Sha256Digest,
    pub raw_baseline_identity_digest: Sha256Digest,
    pub raw_baseline_receipt_digest: Sha256Digest,
    pub assembly_manifest_digest: Sha256Digest,
    pub baseline_state_digest: Sha256Digest,
    pub baseline_reasoning_contract_digest: Sha256Digest,
    pub baseline_budget: NativeResourceVector,
    pub declared_additional_budget: NativeResourceVector,
    pub strict_candidate_guarded_bound: NativeResourceVector,
    pub fallback_reserve: NativeResourceVector,
    pub causal_slack: NativeResourceVector,
    pub actions: Vec<VerifiedReinvestmentActionRecord>,
    pub cost_position: ReinvestmentCostPosition,
    pub plan_digest: Sha256Digest,
}

impl ReinvestmentPlanRecord {
    pub fn validate(&self) -> Result<(), ReinvestmentError> {
        if self.contract_version != REINVESTMENT_CONTRACT_VERSION {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::SchemaVersionMismatch,
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
                ReinvestmentFailureCode::BudgetMismatch,
                "stored causal slack or cost position differs from checked coordinate arithmetic",
            ));
        }
        if self.plan_digest != plan_digest(self)? {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::DigestMismatch,
                "reinvestment plan digest mismatch",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReinvestmentError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ReinvestmentError> {
        if bytes.len() > REINVESTMENT_MAX_CANONICAL_BYTES {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::CanonicalPayloadTooLarge,
                "reinvestment plan exceeds the canonical byte bound",
            ));
        }
        let record: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        record.validate()?;
        if record.canonical_bytes()? != bytes {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::NonCanonicalEncoding,
                "reinvestment plan is not canonical sorted-key JSON",
            ));
        }
        Ok(record)
    }
}

/// Opaque resource admission for one isolated portfolio. It cannot publish.
pub struct ReinvestmentPlanAuthority {
    record: ReinvestmentPlanRecord,
}

impl ReinvestmentPlanAuthority {
    pub fn record(&self) -> &ReinvestmentPlanRecord {
        &self.record
    }

    pub const fn digest(&self) -> Sha256Digest {
        self.record.plan_digest
    }

    pub const fn permits_publication(&self) -> bool {
        false
    }
}

#[allow(clippy::too_many_arguments)]
pub fn admit_reinvestment_plan(
    scope_digest: Sha256Digest,
    comparison_identity_digest: Sha256Digest,
    raw_baseline_identity_digest: Sha256Digest,
    raw_baseline_receipt_digest: Sha256Digest,
    assembly_manifest_digest: Sha256Digest,
    baseline_state_digest: Sha256Digest,
    baseline_reasoning_contract_digest: Sha256Digest,
    baseline_budget: NativeResourceVector,
    declared_additional_budget: NativeResourceVector,
    strict_candidate_guarded_bound: NativeResourceVector,
    fallback_reserve: NativeResourceVector,
    actions: Vec<VerifiedReinvestmentAction>,
) -> Result<ReinvestmentPlanAuthority, ReinvestmentError> {
    if actions.is_empty() || actions.len() > REINVESTMENT_MAX_ACTIONS {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::InvalidPortfolio,
            "reinvestment portfolio must contain 1..=128 actions",
        ));
    }
    let mut action_records = actions
        .into_iter()
        .map(|action| action.record)
        .collect::<Vec<_>>();
    action_records.sort_by_key(|record| record.claim.action_digest);
    let mut record = ReinvestmentPlanRecord {
        contract_version: REINVESTMENT_CONTRACT_VERSION,
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
        causal_slack: NativeResourceVector {
            coordinates: Vec::new(),
        },
        actions: action_records,
        cost_position: ReinvestmentCostPosition::WithinRawBaseline,
        plan_digest: Sha256Digest::ZERO,
    };
    validate_plan_vectors(&record)?;
    validate_action_records(&record)?;
    let (causal_slack, cost_position) = compute_plan_accounting(&record)?;
    record.causal_slack = causal_slack;
    record.cost_position = cost_position;
    record.plan_digest = plan_digest(&record)?;
    record.validate()?;
    Ok(ReinvestmentPlanAuthority { record })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReinvestmentBranchRecord {
    pub contract_version: u16,
    pub plan_digest: Sha256Digest,
    pub action_digest: Sha256Digest,
    pub transaction_receipt_digest: Sha256Digest,
    pub transaction_disposition: ReinvestmentTransactionDisposition,
    pub candidate_state_digest: Sha256Digest,
    pub measured_work_receipt_digests: Vec<Sha256Digest>,
    pub measured_work: NativeResourceVector,
    pub quality_admission_digest: Sha256Digest,
    pub quality_selection: QualitySelection,
    pub strict_protected_improvement: bool,
    pub branch_digest: Sha256Digest,
}

impl ReinvestmentBranchRecord {
    pub fn validate(&self) -> Result<(), ReinvestmentError> {
        if self.contract_version != REINVESTMENT_CONTRACT_VERSION {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::SchemaVersionMismatch,
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
            || self
                .measured_work_receipt_digests
                .contains(&Sha256Digest::ZERO)
        {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::IncompleteMeasuredWork,
                "every native coordinate needs one nonzero causal-work receipt digest",
            ));
        }
        self.measured_work.validate()?;
        if self.quality_selection == QualitySelection::Candidate
            && self.transaction_disposition
                != ReinvestmentTransactionDisposition::CandidateCommitted
        {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::TransactionMismatch,
                "a quality-selected branch needs an isolated committed transaction",
            ));
        }
        if self.branch_digest != branch_digest(self)? {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::DigestMismatch,
                "reinvestment branch digest mismatch",
            ));
        }
        Ok(())
    }
}

/// Opaque branch result built from a real transaction, measured work, and G7.
pub struct VerifiedReinvestmentBranch {
    record: ReinvestmentBranchRecord,
}

impl VerifiedReinvestmentBranch {
    pub fn record(&self) -> &ReinvestmentBranchRecord {
        &self.record
    }

    pub fn is_strictly_improved_candidate(&self) -> bool {
        self.record.quality_selection == QualitySelection::Candidate
            && self.record.strict_protected_improvement
            && matches!(
                self.record.transaction_disposition,
                ReinvestmentTransactionDisposition::CandidateCommitted
            )
    }
}

pub fn complete_reinvestment_branch(
    plan: &ReinvestmentPlanAuthority,
    action_digest: Sha256Digest,
    transaction: &TransactionReceipt,
    measured_work: &[CausalWorkReceipt],
    quality_admission: &QualityAdmission,
) -> Result<VerifiedReinvestmentBranch, ReinvestmentError> {
    plan.record.validate()?;
    let action = plan
        .record
        .actions
        .iter()
        .find(|record| record.claim.action_digest == action_digest)
        .ok_or_else(|| {
            reinvestment_error(
                ReinvestmentFailureCode::UnknownAction,
                "branch action is not in the admitted portfolio",
            )
        })?;
    transaction.canonical_bytes().map_err(transaction_error)?;
    quality_admission.validate().map_err(quality_error)?;
    if transaction.action_digest() != action.claim.transaction_action_digest
        || transaction.baseline_state() != plan.record.baseline_state_digest
        || transaction.external_restoration_debt_count() != 0
        || (transaction.disposition() == TransactionDisposition::BaselineRootRecovered
            && transaction.restoration_scope() != RestorationScope::DeclaredEffectClosure)
    {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::TransactionMismatch,
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
            ReinvestmentFailureCode::QualityBindingMismatch,
            "branch quality admission binds another task, comparison, baseline, or candidate",
        ));
    }
    if quality_admission.selection() == QualitySelection::Candidate
        && transaction.disposition() != TransactionDisposition::CandidateCommitted
    {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::TransactionMismatch,
            "quality selected a candidate that the isolated transaction did not commit",
        ));
    }
    let (measured_vector, receipt_digests) = measured_work_vector(
        measured_work,
        plan.record.assembly_manifest_digest,
        &action.claim.reserved_cost,
    )?;
    let mut record = ReinvestmentBranchRecord {
        contract_version: REINVESTMENT_CONTRACT_VERSION,
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
        branch_digest: Sha256Digest::ZERO,
    };
    record.branch_digest = branch_digest(&record)?;
    record.validate()?;
    Ok(VerifiedReinvestmentBranch { record })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReinvestmentSelectionClaim {
    pub schema_version: String,
    pub plan_digest: Sha256Digest,
    pub selected_action_digest: Sha256Digest,
    pub selected_branch_digest: Sha256Digest,
    pub selected_quality_admission_digest: Sha256Digest,
    pub branch_digests: Vec<Sha256Digest>,
    pub selection_basis: PortfolioSelectionBasis,
    pub dominance_relation_digest: Sha256Digest,
    pub verifier_identity_digest: Sha256Digest,
}

impl ReinvestmentSelectionClaim {
    pub fn validate(&self) -> Result<(), ReinvestmentError> {
        if self.schema_version != REINVESTMENT_SELECTION_SCHEMA_VERSION {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::SchemaVersionMismatch,
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
            || self.branch_digests.len() > REINVESTMENT_MAX_ACTIONS
            || self.branch_digests.contains(&Sha256Digest::ZERO)
            || !strictly_sorted_unique(&self.branch_digests)
            || self
                .branch_digests
                .binary_search(&self.selected_branch_digest)
                .is_err()
        {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::InvalidPortfolio,
                "selection branch digests must be nonempty, unique, and sorted",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReinvestmentError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<Sha256Digest, ReinvestmentError> {
        self.validate()?;
        digest_serializable(SELECTION_CLAIM_DOMAIN, self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReinvestmentSelectionRecord {
    pub contract_version: u16,
    pub claim: ReinvestmentSelectionClaim,
    pub claim_digest: Sha256Digest,
    pub evidence_digest: Sha256Digest,
    pub authority_digest: Sha256Digest,
}

impl ReinvestmentSelectionRecord {
    pub fn validate(&self) -> Result<(), ReinvestmentError> {
        if self.contract_version != REINVESTMENT_CONTRACT_VERSION {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::SchemaVersionMismatch,
                "reinvestment selection contract version mismatch",
            ));
        }
        self.claim.validate()?;
        if self.claim_digest != self.claim.digest()? {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::DigestMismatch,
                "reinvestment selection claim digest mismatch",
            ));
        }
        require_nonzero(
            "reinvestment selection",
            &[self.evidence_digest, self.authority_digest],
        )?;
        if self.authority_digest != selection_authority_digest(self)? {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::DigestMismatch,
                "reinvestment selection authority digest mismatch",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReinvestmentError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ReinvestmentError> {
        if bytes.len() > REINVESTMENT_MAX_CANONICAL_BYTES {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::CanonicalPayloadTooLarge,
                "reinvestment selection exceeds the canonical byte bound",
            ));
        }
        let record: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        record.validate()?;
        if record.canonical_bytes()? != bytes {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::NonCanonicalEncoding,
                "reinvestment selection is not canonical sorted-key JSON",
            ));
        }
        Ok(record)
    }
}

/// Opaque portfolio decision. A later G8/G9 gate must still authorize publish.
pub struct ReinvestmentSelectionAuthority {
    record: ReinvestmentSelectionRecord,
}

impl ReinvestmentSelectionAuthority {
    pub fn record(&self) -> &ReinvestmentSelectionRecord {
        &self.record
    }

    pub const fn selected_action_digest(&self) -> Sha256Digest {
        self.record.claim.selected_action_digest
    }

    pub const fn selected_branch_digest(&self) -> Sha256Digest {
        self.record.claim.selected_branch_digest
    }

    pub const fn selected_quality_admission_digest(&self) -> Sha256Digest {
        self.record.claim.selected_quality_admission_digest
    }

    pub const fn permits_publication(&self) -> bool {
        false
    }
}

pub fn reinvestment_selection_claim(
    plan: &ReinvestmentPlanAuthority,
    branches: &[&VerifiedReinvestmentBranch],
    selected_action_digest: Sha256Digest,
    selection_basis: PortfolioSelectionBasis,
    dominance_relation_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
) -> Result<ReinvestmentSelectionClaim, ReinvestmentError> {
    let mut branch_records = complete_branch_set_ref(plan, branches)?;
    let selected = branch_records
        .iter()
        .find(|branch| branch.action_digest == selected_action_digest)
        .ok_or_else(|| {
            reinvestment_error(
                ReinvestmentFailureCode::UnknownAction,
                "selected action has no completed branch",
            )
        })?;
    if selected.quality_selection != QualitySelection::Candidate
        || !selected.strict_protected_improvement
        || selected.transaction_disposition
            != ReinvestmentTransactionDisposition::CandidateCommitted
    {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::NoProtectedMarginalGain,
            "selected reinvestment branch lacks strict protected quality gain",
        ));
    }
    let selected_branch_digest = selected.branch_digest;
    let selected_quality_admission_digest = selected.quality_admission_digest;
    branch_records.sort_by_key(|branch| branch.branch_digest);
    let claim = ReinvestmentSelectionClaim {
        schema_version: REINVESTMENT_SELECTION_SCHEMA_VERSION.into(),
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

pub fn select_reinvestment_winner(
    plan: &ReinvestmentPlanAuthority,
    branches: Vec<VerifiedReinvestmentBranch>,
    claim: ReinvestmentSelectionClaim,
    dominance_evidence: &VerifiedEvidence<'_, '_>,
) -> Result<ReinvestmentSelectionAuthority, ReinvestmentError> {
    let branch_refs = branches.iter().collect::<Vec<_>>();
    let expected = reinvestment_selection_claim(
        plan,
        &branch_refs,
        claim.selected_action_digest,
        claim.selection_basis,
        claim.dominance_relation_digest,
        claim.verifier_identity_digest,
    )?;
    if claim != expected {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::ActionBindingMismatch,
            "selection claim does not bind the complete admitted branch set",
        ));
    }
    verify_exact_successful_payload(&claim.canonical_bytes()?, dominance_evidence)
        .map_err(q99_error)?;
    if q99_verifier_identity(dominance_evidence) != claim.verifier_identity_digest {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::VerifierIdentityMismatch,
            "dominance verifier identity does not match the selection claim",
        ));
    }
    let mut record = ReinvestmentSelectionRecord {
        contract_version: REINVESTMENT_CONTRACT_VERSION,
        claim_digest: claim.digest()?,
        evidence_digest: verified_evidence_digest(dominance_evidence).map_err(q99_error)?,
        claim,
        authority_digest: Sha256Digest::ZERO,
    };
    record.authority_digest = selection_authority_digest(&record)?;
    record.validate()?;
    Ok(ReinvestmentSelectionAuthority { record })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReinvestmentBaselineRecord {
    pub contract_version: u16,
    pub plan_digest: Sha256Digest,
    pub branch_digests: Vec<Sha256Digest>,
    pub reason: ReinvestmentBaselineReason,
    pub baseline_digest: Sha256Digest,
}

impl ReinvestmentBaselineRecord {
    pub fn validate(&self) -> Result<(), ReinvestmentError> {
        if self.contract_version != REINVESTMENT_CONTRACT_VERSION
            || self.plan_digest == Sha256Digest::ZERO
            || self.branch_digests.is_empty()
            || self.branch_digests.contains(&Sha256Digest::ZERO)
            || !strictly_sorted_unique(&self.branch_digests)
        {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::InvalidPortfolio,
                "baseline reinvestment record is incomplete or unsorted",
            ));
        }
        if self.baseline_digest != baseline_record_digest(self)? {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::DigestMismatch,
                "reinvestment baseline record digest mismatch",
            ));
        }
        Ok(())
    }
}

pub fn fall_back_reinvestment_portfolio(
    plan: &ReinvestmentPlanAuthority,
    branches: Vec<VerifiedReinvestmentBranch>,
    reason: ReinvestmentBaselineReason,
) -> Result<ReinvestmentBaselineRecord, ReinvestmentError> {
    let mut records = complete_branch_set(plan, branches)?;
    records.sort_by_key(|record| record.branch_digest);
    let mut baseline = ReinvestmentBaselineRecord {
        contract_version: REINVESTMENT_CONTRACT_VERSION,
        plan_digest: plan.digest(),
        branch_digests: records.iter().map(|record| record.branch_digest).collect(),
        reason,
        baseline_digest: Sha256Digest::ZERO,
    };
    baseline.baseline_digest = baseline_record_digest(&baseline)?;
    baseline.validate()?;
    Ok(baseline)
}

pub fn reinvestment_contract_manifest() -> Value {
    json!({
        "action_isolation": "successful_exact_verified_evidence_over_canonical_action_claim",
        "arithmetic": "checked_u64_per_frozen_native_coordinate",
        "branch_requirements": [
            "isolated_transaction_receipt", "complete_measured_causal_work",
            "quality_readmission_against_frozen_raw_baseline"
        ],
        "canonical_encoding": "sorted_key_json_no_whitespace",
        "contract_version": REINVESTMENT_CONTRACT_VERSION,
        "cost_positions": ["within_raw_baseline", "declared_additional_budget"],
        "fallback": "reserved_before_portfolio_and_selected_on_unresolved_dominance",
        "higher_effort": "unsupported_without_explicit_ordered_cross_class_theorem",
        "linked_contracts": {
            "causal_work": causal_work_contract_digest(),
            "quality": quality_envelope_contract_digest(),
            "reasoning": reasoning_contract_digest(),
            "transaction": transaction_contract_digest(),
        },
        "negative_space": [
            "predicted_gain_as_measured_gain",
            "more_compute_as_monotone_quality",
            "distributional_evidence_as_individual_selection",
            "unmeasured_work_as_zero",
            "cross_coordinate_resource_substitution",
            "reinvestment_record_as_direct_publication_authority",
        ],
        "published_plan_schema_sha256": REINVESTMENT_PLAN_SCHEMA_SHA256,
        "published_selection_schema_sha256": REINVESTMENT_SELECTION_SCHEMA_SHA256,
        "selection": "exact_verified_maximum_or_pairwise_dominance_and_strict_protected_gain",
    })
}

pub fn reinvestment_contract_digest() -> Sha256Digest {
    digest_value(CONTRACT_DOMAIN, &reinvestment_contract_manifest())
}

fn validate_plan_vectors(record: &ReinvestmentPlanRecord) -> Result<(), ReinvestmentError> {
    record.baseline_budget.validate()?;
    require_same_coordinates(&record.baseline_budget, &record.declared_additional_budget)?;
    require_same_coordinates(
        &record.baseline_budget,
        &record.strict_candidate_guarded_bound,
    )?;
    require_same_coordinates(&record.baseline_budget, &record.fallback_reserve)?;
    if !record.fallback_reserve.any_nonzero() {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::MissingFallbackReserve,
            "amplify mode must retain a nonzero raw-baseline fallback reserve",
        ));
    }
    Ok(())
}

fn validate_action_records(record: &ReinvestmentPlanRecord) -> Result<(), ReinvestmentError> {
    if record.actions.is_empty() || record.actions.len() > REINVESTMENT_MAX_ACTIONS {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::InvalidPortfolio,
            "reinvestment portfolio must contain 1..=128 actions",
        ));
    }
    let mut prior = None;
    for action in &record.actions {
        action.validate()?;
        let claim = &action.claim;
        if prior.is_some_and(|previous| previous >= claim.action_digest) {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::DuplicateAction,
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
                ReinvestmentFailureCode::ActionBindingMismatch,
                "portfolio action binds another scope, comparison, baseline, assembly, state, or reasoning contract",
            ));
        }
        require_same_coordinates(&record.baseline_budget, &claim.reserved_cost)?;
    }
    Ok(())
}

fn compute_plan_accounting(
    record: &ReinvestmentPlanRecord,
) -> Result<(NativeResourceVector, ReinvestmentCostPosition), ReinvestmentError> {
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
        return Ok((causal_slack, ReinvestmentCostPosition::WithinRawBaseline));
    }
    let expanded = record
        .baseline_budget
        .checked_add(&record.declared_additional_budget)?;
    if committed.componentwise_le(&expanded)? {
        return Ok((
            causal_slack,
            ReinvestmentCostPosition::DeclaredAdditionalBudget,
        ));
    }
    Err(reinvestment_error(
        ReinvestmentFailureCode::BudgetExceeded,
        "candidate, fallback reserve, and reinvestment exceed baseline plus declared additional budget",
    ))
}

fn complete_branch_set(
    plan: &ReinvestmentPlanAuthority,
    branches: Vec<VerifiedReinvestmentBranch>,
) -> Result<Vec<ReinvestmentBranchRecord>, ReinvestmentError> {
    plan.record.validate()?;
    if branches.len() != plan.record.actions.len() {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::IncompletePortfolio,
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
                ReinvestmentFailureCode::IncompletePortfolio,
                "branch set is duplicated, incomplete, or bound to another plan",
            ));
        }
    }
    Ok(records)
}

fn complete_branch_set_ref(
    plan: &ReinvestmentPlanAuthority,
    branches: &[&VerifiedReinvestmentBranch],
) -> Result<Vec<ReinvestmentBranchRecord>, ReinvestmentError> {
    plan.record.validate()?;
    if branches.len() != plan.record.actions.len() {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::IncompletePortfolio,
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
                ReinvestmentFailureCode::IncompletePortfolio,
                "branch set is duplicated, incomplete, or bound to another plan",
            ));
        }
    }
    Ok(records)
}

fn measured_work_vector(
    receipts: &[CausalWorkReceipt],
    assembly_manifest_digest: Sha256Digest,
    reserved: &NativeResourceVector,
) -> Result<(NativeResourceVector, Vec<Sha256Digest>), ReinvestmentError> {
    if receipts.len() != reserved.coordinates.len() {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::IncompleteMeasuredWork,
            "every reserved native coordinate needs one causal-work receipt",
        ));
    }
    let mut entries = Vec::with_capacity(receipts.len());
    for receipt in receipts {
        receipt.validate().map_err(causal_work_error)?;
        if receipt.assembly_manifest_digest != assembly_manifest_digest {
            return Err(reinvestment_error(
                ReinvestmentFailureCode::WorkBindingMismatch,
                "causal-work receipt binds another assembly manifest",
            ));
        }
        entries.push((
            resource_identity_digest(&receipt.measurement.identity)?,
            NativeResourceAmount {
                identity: receipt.measurement.identity.clone(),
                amount: receipt.observed_total,
            },
            receipt.receipt_digest,
        ));
    }
    entries.sort_by_key(|entry| entry.0);
    let measured =
        NativeResourceVector::new(entries.iter().map(|entry| entry.1.clone()).collect())?;
    if !measured.same_coordinates(reserved)? || !measured.componentwise_le(reserved)? {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::WorkBoundExceeded,
            "measured reinvestment work exceeds or differs from its reserved native coordinates",
        ));
    }
    Ok((measured, entries.into_iter().map(|entry| entry.2).collect()))
}

fn resource_identity_digest(
    identity: &ParentCounterIdentity,
) -> Result<Sha256Digest, ReinvestmentError> {
    validate_counter_identity(identity)?;
    digest_serializable(RESOURCE_IDENTITY_DOMAIN, identity)
}

fn validate_counter_identity(identity: &ParentCounterIdentity) -> Result<(), ReinvestmentError> {
    if identity.counter_id.is_empty()
        || identity.counter_id.len() > REINVESTMENT_MAX_ID_BYTES
        || identity.boundary_digest == Sha256Digest::ZERO
        || identity.adapter_digest == Sha256Digest::ZERO
        || identity.platform_profile_digest == Sha256Digest::ZERO
    {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::InvalidResourceVector,
            "native resource counter identity is empty, oversized, or incomplete",
        ));
    }
    Ok(())
}

fn require_same_coordinates(
    left: &NativeResourceVector,
    right: &NativeResourceVector,
) -> Result<(), ReinvestmentError> {
    if !left.same_coordinates(right)? {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::ResourceCoordinateMismatch,
            "native resource vectors use different frozen counter coordinates",
        ));
    }
    Ok(())
}

fn action_authority_digest(
    record: &VerifiedReinvestmentActionRecord,
) -> Result<Sha256Digest, ReinvestmentError> {
    Ok(digest_value(
        ACTION_AUTHORITY_DOMAIN,
        &json!({
            "claim_digest": record.claim_digest,
            "evidence_digest": record.evidence_digest,
        }),
    ))
}

fn plan_digest(record: &ReinvestmentPlanRecord) -> Result<Sha256Digest, ReinvestmentError> {
    Ok(digest_value(
        PLAN_DOMAIN,
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

fn branch_digest(record: &ReinvestmentBranchRecord) -> Result<Sha256Digest, ReinvestmentError> {
    Ok(digest_value(
        BRANCH_DOMAIN,
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
    record: &ReinvestmentSelectionRecord,
) -> Result<Sha256Digest, ReinvestmentError> {
    Ok(digest_value(
        SELECTION_AUTHORITY_DOMAIN,
        &json!({
            "claim_digest": record.claim_digest,
            "contract_version": record.contract_version,
            "evidence_digest": record.evidence_digest,
        }),
    ))
}

fn baseline_record_digest(
    record: &ReinvestmentBaselineRecord,
) -> Result<Sha256Digest, ReinvestmentError> {
    Ok(digest_value(
        BASELINE_DOMAIN,
        &json!({
            "branch_digests": record.branch_digests,
            "contract_version": record.contract_version,
            "plan_digest": record.plan_digest,
            "reason": record.reason,
        }),
    ))
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ReinvestmentError> {
    let value = serde_json::to_value(value).map_err(json_error)?;
    Ok(canonical_json(&value).into_bytes())
}

fn digest_serializable<T: Serialize>(
    domain: &[u8],
    value: &T,
) -> Result<Sha256Digest, ReinvestmentError> {
    Ok(domain_digest(domain, &canonical_bytes(value)?))
}

fn digest_value(domain: &[u8], value: &Value) -> Sha256Digest {
    domain_digest(domain, canonical_json(value).as_bytes())
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> Sha256Digest {
    let mut input = Vec::with_capacity(domain.len() + bytes.len());
    input.extend_from_slice(domain);
    input.extend_from_slice(bytes);
    Sha256Digest::from_bytes(zero_abi::sha256(&input))
}

fn require_nonzero(label: &'static str, values: &[Sha256Digest]) -> Result<(), ReinvestmentError> {
    if values.contains(&Sha256Digest::ZERO) {
        return Err(reinvestment_error(
            ReinvestmentFailureCode::ZeroDigest,
            format!("{label} contains a zero digest"),
        ));
    }
    Ok(())
}

fn strictly_sorted_unique(values: &[Sha256Digest]) -> bool {
    values.windows(2).all(|window| window[0] < window[1])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReinvestmentFailureCode {
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
pub struct ReinvestmentError {
    code: ReinvestmentFailureCode,
    detail: String,
}

impl ReinvestmentError {
    pub const fn failure_code(&self) -> ReinvestmentFailureCode {
        self.code
    }
}

impl fmt::Display for ReinvestmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "reinvestment {:?}: {}", self.code, self.detail)
    }
}

impl Error for ReinvestmentError {}

fn reinvestment_error(
    code: ReinvestmentFailureCode,
    detail: impl Into<String>,
) -> ReinvestmentError {
    ReinvestmentError {
        code,
        detail: detail.into(),
    }
}

fn json_error(error: serde_json::Error) -> ReinvestmentError {
    reinvestment_error(ReinvestmentFailureCode::InvalidJson, error.to_string())
}

fn q99_error(error: crate::q99::Q99Error) -> ReinvestmentError {
    reinvestment_error(
        ReinvestmentFailureCode::EvidencePayloadMismatch,
        error.to_string(),
    )
}

fn reasoning_error(error: zero_abi::ReasoningContractError) -> ReinvestmentError {
    reinvestment_error(
        ReinvestmentFailureCode::UnsupportedReasoningChange,
        error.to_string(),
    )
}

fn transaction_error(error: crate::transaction::TransactionError) -> ReinvestmentError {
    reinvestment_error(
        ReinvestmentFailureCode::DependencyFailure,
        error.to_string(),
    )
}

fn quality_error(error: crate::quality::QualityEnvelopeError) -> ReinvestmentError {
    reinvestment_error(
        ReinvestmentFailureCode::DependencyFailure,
        error.to_string(),
    )
}

fn causal_work_error(error: zero_ledger::CausalWorkError) -> ReinvestmentError {
    reinvestment_error(
        ReinvestmentFailureCode::DependencyFailure,
        error.to_string(),
    )
}
