//! Proof-carrying protected-quality envelope for strict candidate admission.
//!
//! The envelope keeps exact, pointwise, scoped-class, and distributional evidence
//! distinct. Strict publication admits only evidence that protects the current
//! task pointwise. Distributional and unidentified candidates select the frozen
//! raw baseline instead of laundering a population claim into an individual one.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zero_abi::{Sha256Digest, canonical_json, sha256};
use zero_cert::VerifiedEvidence;

pub const QUALITY_ENVELOPE_CONTRACT_VERSION: u16 = 1;
pub const QUALITY_ENVELOPE_MAX_CANONICAL_BYTES: usize = 1_048_576;
pub const QUALITY_ENVELOPE_MAX_DIMENSIONS: usize = 128;
pub const QUALITY_ENVELOPE_MAX_METRIC_ID_BYTES: usize = 128;
pub const QUALITY_PPM_SCALE: i64 = 1_000_000;

const PAIR_DOMAIN: &[u8] = b"zerostack.quality.pair.v1\0";
const EXACT_DOMAIN: &[u8] = b"zerostack.quality.exact_neutral.v1\0";
const POINTWISE_DOMAIN: &[u8] = b"zerostack.quality.pointwise.v1\0";
const CLASS_RULE_DOMAIN: &[u8] = b"zerostack.quality.class_rule.v1\0";
const MEMBERSHIP_DOMAIN: &[u8] = b"zerostack.quality.membership.v1\0";
const SCOPED_DOMAIN: &[u8] = b"zerostack.quality.scoped.v1\0";
const DISTRIBUTIONAL_CLAIM_DOMAIN: &[u8] = b"zerostack.quality.distributional_claim.v1\0";
const DISTRIBUTIONAL_DOMAIN: &[u8] = b"zerostack.quality.distributional.v1\0";
const VERIFIER_DOMAIN: &[u8] = b"zerostack.quality.verifier.v1\0";
const ADMISSION_DOMAIN: &[u8] = b"zerostack.quality.admission.v1\0";
const CONTRACT_DOMAIN: &[u8] = b"zerostack.quality.contract.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricOrder {
    AtLeast,
    AtMost,
    Exact,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedMetric {
    pub metric_id: String,
    pub order: MetricOrder,
    pub baseline_value: i64,
    pub candidate_value: i64,
}

impl ProtectedMetric {
    fn validate(&self) -> Result<(), QualityEnvelopeError> {
        validate_id("metric_id", &self.metric_id)?;
        if self.no_worse() {
            Ok(())
        } else {
            Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::CandidateRegression,
                format!("candidate regresses protected metric {}", self.metric_id),
            ))
        }
    }

    pub const fn no_worse(&self) -> bool {
        match self.order {
            MetricOrder::AtLeast => self.candidate_value >= self.baseline_value,
            MetricOrder::AtMost => self.candidate_value <= self.baseline_value,
            MetricOrder::Exact => self.candidate_value == self.baseline_value,
        }
    }

    pub const fn strictly_better(&self) -> bool {
        match self.order {
            MetricOrder::AtLeast => self.candidate_value > self.baseline_value,
            MetricOrder::AtMost => self.candidate_value < self.baseline_value,
            MetricOrder::Exact => false,
        }
    }
}

/// Canonical paired protected outcomes for one task and comparison identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualityPair {
    contract_version: u16,
    task_digest: Sha256Digest,
    comparison_identity_digest: Sha256Digest,
    raw_baseline_identity_digest: Sha256Digest,
    candidate_identity_digest: Sha256Digest,
    baseline_outcome_digest: Sha256Digest,
    candidate_outcome_digest: Sha256Digest,
    protected_schema_digest: Sha256Digest,
    pairing_method_digest: Sha256Digest,
    dimensions: Vec<ProtectedMetric>,
}

impl QualityPair {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_digest: Sha256Digest,
        comparison_identity_digest: Sha256Digest,
        raw_baseline_identity_digest: Sha256Digest,
        candidate_identity_digest: Sha256Digest,
        baseline_outcome_digest: Sha256Digest,
        candidate_outcome_digest: Sha256Digest,
        protected_schema_digest: Sha256Digest,
        pairing_method_digest: Sha256Digest,
        dimensions: Vec<ProtectedMetric>,
    ) -> Result<Self, QualityEnvelopeError> {
        let pair = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION,
            task_digest,
            comparison_identity_digest,
            raw_baseline_identity_digest,
            candidate_identity_digest,
            baseline_outcome_digest,
            candidate_outcome_digest,
            protected_schema_digest,
            pairing_method_digest,
            dimensions,
        };
        pair.validate()?;
        Ok(pair)
    }

    pub fn validate(&self) -> Result<(), QualityEnvelopeError> {
        if self.contract_version != QUALITY_ENVELOPE_CONTRACT_VERSION {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::SchemaVersionMismatch,
                "quality-pair contract version is not current",
            ));
        }
        require_nonzero(
            "quality pair",
            &[
                self.task_digest,
                self.comparison_identity_digest,
                self.raw_baseline_identity_digest,
                self.candidate_identity_digest,
                self.baseline_outcome_digest,
                self.candidate_outcome_digest,
                self.protected_schema_digest,
                self.pairing_method_digest,
            ],
        )?;
        if self.dimensions.is_empty() || self.dimensions.len() > QUALITY_ENVELOPE_MAX_DIMENSIONS
        {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::InvalidProtectedVector,
                "protected vector is empty or exceeds its bound",
            ));
        }
        let mut previous: Option<&str> = None;
        for metric in &self.dimensions {
            metric.validate()?;
            if previous.is_some_and(|value| value >= metric.metric_id.as_str()) {
                return Err(QualityEnvelopeError::new(
                    QualityEnvelopeFailureCode::NonCanonicalOrder,
                    "protected metric ids must be unique and strictly sorted",
                ));
            }
            previous = Some(&metric.metric_id);
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, QualityEnvelopeError> {
        if bytes.len() > QUALITY_ENVELOPE_MAX_CANONICAL_BYTES {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::CanonicalPayloadTooLarge,
                "quality pair exceeds the canonical byte bound",
            ));
        }
        let pair: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        pair.validate()?;
        if pair.canonical_bytes()? != bytes {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::NonCanonicalEncoding,
                "quality pair bytes are not canonical sorted-key JSON",
            ));
        }
        Ok(pair)
    }

    pub fn digest(&self) -> Result<Sha256Digest, QualityEnvelopeError> {
        Ok(domain_digest(PAIR_DOMAIN, &self.canonical_bytes()?))
    }

    pub const fn task_digest(&self) -> Sha256Digest {
        self.task_digest
    }
    pub const fn comparison_identity_digest(&self) -> Sha256Digest {
        self.comparison_identity_digest
    }
    pub const fn raw_baseline_identity_digest(&self) -> Sha256Digest {
        self.raw_baseline_identity_digest
    }
    pub const fn baseline_outcome_digest(&self) -> Sha256Digest {
        self.baseline_outcome_digest
    }
    pub const fn candidate_outcome_digest(&self) -> Sha256Digest {
        self.candidate_outcome_digest
    }
    pub fn strictly_better(&self) -> bool {
        self.dimensions
            .iter()
            .any(ProtectedMetric::strictly_better)
    }
}

/// Equality-by-substitution certificate. The constructor requires both sides
/// of every protected continuation identity to match exactly.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExactNeutralCertificate {
    contract_version: u16,
    task_digest: Sha256Digest,
    comparison_identity_digest: Sha256Digest,
    raw_baseline_identity_digest: Sha256Digest,
    candidate_identity_digest: Sha256Digest,
    continuation_identity_digest: Sha256Digest,
    model_visible_input_digest: Sha256Digest,
    protected_outcome_digest: Sha256Digest,
    certificate_digest: Sha256Digest,
}

impl ExactNeutralCertificate {
    #[allow(clippy::too_many_arguments)]
    pub fn verify(
        task_digest: Sha256Digest,
        comparison_identity_digest: Sha256Digest,
        raw_baseline_identity_digest: Sha256Digest,
        candidate_identity_digest: Sha256Digest,
        baseline_continuation_identity_digest: Sha256Digest,
        candidate_continuation_identity_digest: Sha256Digest,
        baseline_model_visible_input_digest: Sha256Digest,
        candidate_model_visible_input_digest: Sha256Digest,
        baseline_protected_outcome_digest: Sha256Digest,
        candidate_protected_outcome_digest: Sha256Digest,
    ) -> Result<Self, QualityEnvelopeError> {
        require_nonzero(
            "exact-neutral certificate",
            &[
                task_digest,
                comparison_identity_digest,
                raw_baseline_identity_digest,
                candidate_identity_digest,
                baseline_continuation_identity_digest,
                candidate_continuation_identity_digest,
                baseline_model_visible_input_digest,
                candidate_model_visible_input_digest,
                baseline_protected_outcome_digest,
                candidate_protected_outcome_digest,
            ],
        )?;
        if baseline_continuation_identity_digest != candidate_continuation_identity_digest
            || baseline_model_visible_input_digest != candidate_model_visible_input_digest
            || baseline_protected_outcome_digest != candidate_protected_outcome_digest
        {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::ExactNeutralMismatch,
                "continuation, model-visible input, and protected outcome must all match",
            ));
        }
        let mut certificate = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION,
            task_digest,
            comparison_identity_digest,
            raw_baseline_identity_digest,
            candidate_identity_digest,
            continuation_identity_digest: baseline_continuation_identity_digest,
            model_visible_input_digest: baseline_model_visible_input_digest,
            protected_outcome_digest: baseline_protected_outcome_digest,
            certificate_digest: Sha256Digest::ZERO,
        };
        certificate.certificate_digest = certificate.expected_digest()?;
        Ok(certificate)
    }

    pub fn validate(&self) -> Result<(), QualityEnvelopeError> {
        require_version_and_digest(
            self.contract_version,
            self.certificate_digest,
            self.expected_digest()?,
            "exact-neutral certificate",
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeError> {
        self.validate()?;
        canonical_bytes(self)
    }

    fn expected_digest(&self) -> Result<Sha256Digest, QualityEnvelopeError> {
        digest_body(
            EXACT_DOMAIN,
            json!({
                "candidate_identity_digest": self.candidate_identity_digest,
                "comparison_identity_digest": self.comparison_identity_digest,
                "continuation_identity_digest": self.continuation_identity_digest,
                "contract_version": self.contract_version,
                "model_visible_input_digest": self.model_visible_input_digest,
                "protected_outcome_digest": self.protected_outcome_digest,
                "raw_baseline_identity_digest": self.raw_baseline_identity_digest,
                "task_digest": self.task_digest,
            }),
        )
    }
}

/// Opaque, current-task proof that the candidate protected vector is no worse.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PointwiseDominanceCertificate {
    contract_version: u16,
    task_digest: Sha256Digest,
    comparison_identity_digest: Sha256Digest,
    raw_baseline_identity_digest: Sha256Digest,
    candidate_identity_digest: Sha256Digest,
    baseline_outcome_digest: Sha256Digest,
    candidate_outcome_digest: Sha256Digest,
    pair_digest: Sha256Digest,
    pairing_method_digest: Sha256Digest,
    protected_predicate_digest: Sha256Digest,
    evidence_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
    strictly_better: bool,
    certificate_digest: Sha256Digest,
}

impl PointwiseDominanceCertificate {
    pub fn verify(
        pair: &QualityPair,
        protected_predicate_digest: Sha256Digest,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, QualityEnvelopeError> {
        pair.validate()?;
        if protected_predicate_digest == Sha256Digest::ZERO {
            return Err(missing_binding("protected predicate"));
        }
        let pair_bytes = pair.canonical_bytes()?;
        require_exact_payload("pointwise pair", &pair_bytes, evidence)?;
        let mut certificate = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION,
            task_digest: pair.task_digest,
            comparison_identity_digest: pair.comparison_identity_digest,
            raw_baseline_identity_digest: pair.raw_baseline_identity_digest,
            candidate_identity_digest: pair.candidate_identity_digest,
            baseline_outcome_digest: pair.baseline_outcome_digest,
            candidate_outcome_digest: pair.candidate_outcome_digest,
            pair_digest: pair.digest()?,
            pairing_method_digest: pair.pairing_method_digest,
            protected_predicate_digest,
            evidence_digest: evidence_digest(evidence)?,
            verifier_identity_digest: verifier_identity_digest(evidence)?,
            strictly_better: pair.strictly_better(),
            certificate_digest: Sha256Digest::ZERO,
        };
        certificate.certificate_digest = certificate.expected_digest()?;
        Ok(certificate)
    }

    pub fn validate(&self) -> Result<(), QualityEnvelopeError> {
        require_nonzero(
            "pointwise certificate",
            &[
                self.task_digest,
                self.comparison_identity_digest,
                self.raw_baseline_identity_digest,
                self.candidate_identity_digest,
                self.baseline_outcome_digest,
                self.candidate_outcome_digest,
                self.pair_digest,
                self.pairing_method_digest,
                self.protected_predicate_digest,
                self.evidence_digest,
                self.verifier_identity_digest,
            ],
        )?;
        require_version_and_digest(
            self.contract_version,
            self.certificate_digest,
            self.expected_digest()?,
            "pointwise certificate",
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeError> {
        self.validate()?;
        canonical_bytes(self)
    }

    fn expected_digest(&self) -> Result<Sha256Digest, QualityEnvelopeError> {
        digest_body(
            POINTWISE_DOMAIN,
            json!({
                "baseline_outcome_digest": self.baseline_outcome_digest,
                "candidate_identity_digest": self.candidate_identity_digest,
                "candidate_outcome_digest": self.candidate_outcome_digest,
                "comparison_identity_digest": self.comparison_identity_digest,
                "contract_version": self.contract_version,
                "evidence_digest": self.evidence_digest,
                "pair_digest": self.pair_digest,
                "pairing_method_digest": self.pairing_method_digest,
                "protected_predicate_digest": self.protected_predicate_digest,
                "raw_baseline_identity_digest": self.raw_baseline_identity_digest,
                "strictly_better": self.strictly_better,
                "task_digest": self.task_digest,
                "verifier_identity_digest": self.verifier_identity_digest,
            }),
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DominanceClaim {
    NoWorse,
    StrictlyBetter,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClassDominanceRule {
    contract_version: u16,
    class_digest: Sha256Digest,
    comparison_identity_digest: Sha256Digest,
    protected_schema_digest: Sha256Digest,
    candidate_protocol_digest: Sha256Digest,
    raw_baseline_identity_digest: Sha256Digest,
    dominance_rule_digest: Sha256Digest,
    claim: DominanceClaim,
}

impl ClassDominanceRule {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        class_digest: Sha256Digest,
        comparison_identity_digest: Sha256Digest,
        protected_schema_digest: Sha256Digest,
        candidate_protocol_digest: Sha256Digest,
        raw_baseline_identity_digest: Sha256Digest,
        dominance_rule_digest: Sha256Digest,
        claim: DominanceClaim,
    ) -> Result<Self, QualityEnvelopeError> {
        let rule = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION,
            class_digest,
            comparison_identity_digest,
            protected_schema_digest,
            candidate_protocol_digest,
            raw_baseline_identity_digest,
            dominance_rule_digest,
            claim,
        };
        rule.validate()?;
        Ok(rule)
    }

    fn validate(&self) -> Result<(), QualityEnvelopeError> {
        if self.contract_version != QUALITY_ENVELOPE_CONTRACT_VERSION {
            return Err(version_error("class rule"));
        }
        require_nonzero(
            "class rule",
            &[
                self.class_digest,
                self.comparison_identity_digest,
                self.protected_schema_digest,
                self.candidate_protocol_digest,
                self.raw_baseline_identity_digest,
                self.dominance_rule_digest,
            ],
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<Sha256Digest, QualityEnvelopeError> {
        Ok(domain_digest(
            CLASS_RULE_DOMAIN,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskClassMembership {
    contract_version: u16,
    class_digest: Sha256Digest,
    task_digest: Sha256Digest,
    candidate_protocol_digest: Sha256Digest,
    membership_predicate_digest: Sha256Digest,
}

impl TaskClassMembership {
    pub fn new(
        class_digest: Sha256Digest,
        task_digest: Sha256Digest,
        candidate_protocol_digest: Sha256Digest,
        membership_predicate_digest: Sha256Digest,
    ) -> Result<Self, QualityEnvelopeError> {
        let membership = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION,
            class_digest,
            task_digest,
            candidate_protocol_digest,
            membership_predicate_digest,
        };
        membership.validate()?;
        Ok(membership)
    }

    fn validate(&self) -> Result<(), QualityEnvelopeError> {
        if self.contract_version != QUALITY_ENVELOPE_CONTRACT_VERSION {
            return Err(version_error("class membership"));
        }
        require_nonzero(
            "class membership",
            &[
                self.class_digest,
                self.task_digest,
                self.candidate_protocol_digest,
                self.membership_predicate_digest,
            ],
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<Sha256Digest, QualityEnvelopeError> {
        Ok(domain_digest(
            MEMBERSHIP_DOMAIN,
            &self.canonical_bytes()?,
        ))
    }
}

/// Opaque reusable class proof plus exact task-membership proof.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedClassDominanceCertificate {
    contract_version: u16,
    class_digest: Sha256Digest,
    task_digest: Sha256Digest,
    comparison_identity_digest: Sha256Digest,
    raw_baseline_identity_digest: Sha256Digest,
    candidate_protocol_digest: Sha256Digest,
    class_rule_digest: Sha256Digest,
    membership_digest: Sha256Digest,
    class_evidence_digest: Sha256Digest,
    membership_evidence_digest: Sha256Digest,
    class_verifier_identity_digest: Sha256Digest,
    membership_verifier_identity_digest: Sha256Digest,
    claim: DominanceClaim,
    certificate_digest: Sha256Digest,
}

impl ScopedClassDominanceCertificate {
    pub fn verify(
        rule: &ClassDominanceRule,
        membership: &TaskClassMembership,
        class_evidence: &VerifiedEvidence<'_, '_>,
        membership_evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, QualityEnvelopeError> {
        rule.validate()?;
        membership.validate()?;
        if rule.class_digest != membership.class_digest
            || rule.candidate_protocol_digest != membership.candidate_protocol_digest
        {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::ClassMembershipMismatch,
                "class rule and task membership bind different class or candidate protocols",
            ));
        }
        require_exact_payload("class rule", &rule.canonical_bytes()?, class_evidence)?;
        require_exact_payload(
            "class membership",
            &membership.canonical_bytes()?,
            membership_evidence,
        )?;
        let mut certificate = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION,
            class_digest: rule.class_digest,
            task_digest: membership.task_digest,
            comparison_identity_digest: rule.comparison_identity_digest,
            raw_baseline_identity_digest: rule.raw_baseline_identity_digest,
            candidate_protocol_digest: rule.candidate_protocol_digest,
            class_rule_digest: rule.digest()?,
            membership_digest: membership.digest()?,
            class_evidence_digest: evidence_digest(class_evidence)?,
            membership_evidence_digest: evidence_digest(membership_evidence)?,
            class_verifier_identity_digest: verifier_identity_digest(class_evidence)?,
            membership_verifier_identity_digest: verifier_identity_digest(membership_evidence)?,
            claim: rule.claim,
            certificate_digest: Sha256Digest::ZERO,
        };
        certificate.certificate_digest = certificate.expected_digest()?;
        Ok(certificate)
    }

    pub fn validate(&self) -> Result<(), QualityEnvelopeError> {
        require_nonzero(
            "scoped class certificate",
            &[
                self.class_digest,
                self.task_digest,
                self.comparison_identity_digest,
                self.raw_baseline_identity_digest,
                self.candidate_protocol_digest,
                self.class_rule_digest,
                self.membership_digest,
                self.class_evidence_digest,
                self.membership_evidence_digest,
                self.class_verifier_identity_digest,
                self.membership_verifier_identity_digest,
            ],
        )?;
        require_version_and_digest(
            self.contract_version,
            self.certificate_digest,
            self.expected_digest()?,
            "scoped class certificate",
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeError> {
        self.validate()?;
        canonical_bytes(self)
    }

    fn expected_digest(&self) -> Result<Sha256Digest, QualityEnvelopeError> {
        digest_body(
            SCOPED_DOMAIN,
            json!({
                "candidate_protocol_digest": self.candidate_protocol_digest,
                "certificate_version": self.contract_version,
                "class_digest": self.class_digest,
                "class_evidence_digest": self.class_evidence_digest,
                "class_rule_digest": self.class_rule_digest,
                "class_verifier_identity_digest": self.class_verifier_identity_digest,
                "claim": self.claim,
                "comparison_identity_digest": self.comparison_identity_digest,
                "membership_digest": self.membership_digest,
                "membership_evidence_digest": self.membership_evidence_digest,
                "membership_verifier_identity_digest": self.membership_verifier_identity_digest,
                "raw_baseline_identity_digest": self.raw_baseline_identity_digest,
                "task_digest": self.task_digest,
            }),
        )
    }
}

/// Frozen paired-population claim. Integer ppm prevents float/NaN ambiguity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DistributionalClaim {
    contract_version: u16,
    benchmark_digest: Sha256Digest,
    comparison_identity_digest: Sha256Digest,
    candidate_protocol_digest: Sha256Digest,
    raw_baseline_identity_digest: Sha256Digest,
    baseline_outcome_digest: Sha256Digest,
    protected_schema_digest: Sha256Digest,
    pairing_method_digest: Sha256Digest,
    protected_predicate_digest: Sha256Digest,
    paired_tasks: u64,
    candidate_wins: u64,
    protected_losses: u64,
    ties: u64,
    mean_gain_ppm: i64,
    lower_confidence_gain_ppm: i64,
    confidence_ppm: u32,
}

impl DistributionalClaim {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        benchmark_digest: Sha256Digest,
        comparison_identity_digest: Sha256Digest,
        candidate_protocol_digest: Sha256Digest,
        raw_baseline_identity_digest: Sha256Digest,
        baseline_outcome_digest: Sha256Digest,
        protected_schema_digest: Sha256Digest,
        pairing_method_digest: Sha256Digest,
        protected_predicate_digest: Sha256Digest,
        paired_tasks: u64,
        candidate_wins: u64,
        protected_losses: u64,
        ties: u64,
        mean_gain_ppm: i64,
        lower_confidence_gain_ppm: i64,
        confidence_ppm: u32,
    ) -> Result<Self, QualityEnvelopeError> {
        let claim = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION,
            benchmark_digest,
            comparison_identity_digest,
            candidate_protocol_digest,
            raw_baseline_identity_digest,
            baseline_outcome_digest,
            protected_schema_digest,
            pairing_method_digest,
            protected_predicate_digest,
            paired_tasks,
            candidate_wins,
            protected_losses,
            ties,
            mean_gain_ppm,
            lower_confidence_gain_ppm,
            confidence_ppm,
        };
        claim.validate()?;
        Ok(claim)
    }

    fn validate(&self) -> Result<(), QualityEnvelopeError> {
        if self.contract_version != QUALITY_ENVELOPE_CONTRACT_VERSION {
            return Err(version_error("distributional claim"));
        }
        require_nonzero(
            "distributional claim",
            &[
                self.benchmark_digest,
                self.comparison_identity_digest,
                self.candidate_protocol_digest,
                self.raw_baseline_identity_digest,
                self.baseline_outcome_digest,
                self.protected_schema_digest,
                self.pairing_method_digest,
                self.protected_predicate_digest,
            ],
        )?;
        let total = self
            .candidate_wins
            .checked_add(self.protected_losses)
            .and_then(|value| value.checked_add(self.ties));
        if self.paired_tasks == 0 || total != Some(self.paired_tasks) {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::InvalidDistributionalCounts,
                "paired task count must equal wins plus protected losses plus ties",
            ));
        }
        if self.confidence_ppm == 0
            || i64::from(self.confidence_ppm) >= QUALITY_PPM_SCALE
            || !(-QUALITY_PPM_SCALE..=QUALITY_PPM_SCALE).contains(&self.mean_gain_ppm)
            || !(-QUALITY_PPM_SCALE..=QUALITY_PPM_SCALE)
                .contains(&self.lower_confidence_gain_ppm)
            || self.lower_confidence_gain_ppm > self.mean_gain_ppm
        {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::InvalidDistributionalBound,
                "confidence and paired gain ppm values are outside their frozen bounds",
            ));
        }
        let paired_delta = i128::from(self.candidate_wins) - i128::from(self.protected_losses);
        let expected_mean_ppm =
            paired_delta * i128::from(QUALITY_PPM_SCALE) / i128::from(self.paired_tasks);
        if i128::from(self.mean_gain_ppm) != expected_mean_ppm {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::InvalidDistributionalBound,
                "mean gain ppm does not equal the frozen paired win-loss calculation",
            ));
        }
        if self.lower_confidence_gain_ppm <= 0 {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::NonPositiveDistributionalBound,
                "distributional admission requires a positive lower confidence gain",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<Sha256Digest, QualityEnvelopeError> {
        Ok(domain_digest(
            DISTRIBUTIONAL_CLAIM_DOMAIN,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DistributionalCertificate {
    contract_version: u16,
    benchmark_digest: Sha256Digest,
    comparison_identity_digest: Sha256Digest,
    candidate_protocol_digest: Sha256Digest,
    raw_baseline_identity_digest: Sha256Digest,
    baseline_outcome_digest: Sha256Digest,
    pairing_method_digest: Sha256Digest,
    protected_predicate_digest: Sha256Digest,
    claim_digest: Sha256Digest,
    paired_tasks: u64,
    protected_losses: u64,
    lower_confidence_gain_ppm: i64,
    confidence_ppm: u32,
    evidence_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
    certificate_digest: Sha256Digest,
}

impl DistributionalCertificate {
    pub fn verify(
        claim: &DistributionalClaim,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, QualityEnvelopeError> {
        claim.validate()?;
        require_exact_payload("distributional claim", &claim.canonical_bytes()?, evidence)?;
        let mut certificate = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION,
            benchmark_digest: claim.benchmark_digest,
            comparison_identity_digest: claim.comparison_identity_digest,
            candidate_protocol_digest: claim.candidate_protocol_digest,
            raw_baseline_identity_digest: claim.raw_baseline_identity_digest,
            baseline_outcome_digest: claim.baseline_outcome_digest,
            pairing_method_digest: claim.pairing_method_digest,
            protected_predicate_digest: claim.protected_predicate_digest,
            claim_digest: claim.digest()?,
            paired_tasks: claim.paired_tasks,
            protected_losses: claim.protected_losses,
            lower_confidence_gain_ppm: claim.lower_confidence_gain_ppm,
            confidence_ppm: claim.confidence_ppm,
            evidence_digest: evidence_digest(evidence)?,
            verifier_identity_digest: verifier_identity_digest(evidence)?,
            certificate_digest: Sha256Digest::ZERO,
        };
        certificate.certificate_digest = certificate.expected_digest()?;
        Ok(certificate)
    }

    pub fn validate(&self) -> Result<(), QualityEnvelopeError> {
        require_nonzero(
            "distributional certificate",
            &[
                self.benchmark_digest,
                self.comparison_identity_digest,
                self.candidate_protocol_digest,
                self.raw_baseline_identity_digest,
                self.baseline_outcome_digest,
                self.pairing_method_digest,
                self.protected_predicate_digest,
                self.claim_digest,
                self.evidence_digest,
                self.verifier_identity_digest,
            ],
        )?;
        if self.paired_tasks == 0
            || self.confidence_ppm == 0
            || i64::from(self.confidence_ppm) >= QUALITY_PPM_SCALE
            || self.lower_confidence_gain_ppm <= 0
        {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::InvalidDistributionalBound,
                "distributional certificate lost its positive bounded population claim",
            ));
        }
        require_version_and_digest(
            self.contract_version,
            self.certificate_digest,
            self.expected_digest()?,
            "distributional certificate",
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeError> {
        self.validate()?;
        canonical_bytes(self)
    }

    fn expected_digest(&self) -> Result<Sha256Digest, QualityEnvelopeError> {
        digest_body(
            DISTRIBUTIONAL_DOMAIN,
            json!({
                "baseline_outcome_digest": self.baseline_outcome_digest,
                "benchmark_digest": self.benchmark_digest,
                "candidate_protocol_digest": self.candidate_protocol_digest,
                "certificate_version": self.contract_version,
                "claim_digest": self.claim_digest,
                "comparison_identity_digest": self.comparison_identity_digest,
                "confidence_ppm": self.confidence_ppm,
                "evidence_digest": self.evidence_digest,
                "lower_confidence_gain_ppm": self.lower_confidence_gain_ppm,
                "paired_tasks": self.paired_tasks,
                "pairing_method_digest": self.pairing_method_digest,
                "protected_losses": self.protected_losses,
                "protected_predicate_digest": self.protected_predicate_digest,
                "raw_baseline_identity_digest": self.raw_baseline_identity_digest,
                "verifier_identity_digest": self.verifier_identity_digest,
            }),
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnidentifiedReason {
    MissingEvidence,
    BindingMismatch,
    VerifierUnsupported,
    CandidateRegression,
    DistributionalOnly,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "snake_case",
    tag = "evidence_class",
    content = "certificate"
)]
pub enum QualityEvidence {
    ExactNeutral(ExactNeutralCertificate),
    PointwiseDominance(PointwiseDominanceCertificate),
    ScopedClassDominance(ScopedClassDominanceCertificate),
    Distributional(DistributionalCertificate),
    Unidentified {
        scope_digest: Sha256Digest,
        comparison_identity_digest: Sha256Digest,
        candidate_identity_digest: Sha256Digest,
        reason: UnidentifiedReason,
    },
}

impl QualityEvidence {
    pub fn unidentified(
        scope_digest: Sha256Digest,
        comparison_identity_digest: Sha256Digest,
        candidate_identity_digest: Sha256Digest,
        reason: UnidentifiedReason,
    ) -> Result<Self, QualityEnvelopeError> {
        require_nonzero(
            "unidentified quality evidence",
            &[
                scope_digest,
                comparison_identity_digest,
                candidate_identity_digest,
            ],
        )?;
        Ok(Self::Unidentified {
            scope_digest,
            comparison_identity_digest,
            candidate_identity_digest,
            reason,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityEvidenceClass {
    ExactNeutral,
    PointwiseDominance,
    ScopedClassDominance,
    Distributional,
    Unidentified,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualitySelection {
    Candidate,
    FrozenBaseline,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityGuarantee {
    ExactSubstitution,
    PointwiseNoWorse,
    ScopedClassNoWorse,
    DistributionalOnly,
    Unidentified,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenBaseline {
    identity_digest: Sha256Digest,
    protected_outcome_digest: Sha256Digest,
    receipt_digest: Sha256Digest,
}

impl FrozenBaseline {
    pub fn new(
        identity_digest: Sha256Digest,
        protected_outcome_digest: Sha256Digest,
        receipt_digest: Sha256Digest,
    ) -> Result<Self, QualityEnvelopeError> {
        require_nonzero(
            "frozen baseline",
            &[identity_digest, protected_outcome_digest, receipt_digest],
        )?;
        Ok(Self {
            identity_digest,
            protected_outcome_digest,
            receipt_digest,
        })
    }

    pub const fn identity_digest(&self) -> Sha256Digest {
        self.identity_digest
    }
    pub const fn protected_outcome_digest(&self) -> Sha256Digest {
        self.protected_outcome_digest
    }
    pub const fn receipt_digest(&self) -> Sha256Digest {
        self.receipt_digest
    }
}

/// Opaque G7 decision. Construction enforces that population-only evidence
/// cannot authorize an individual candidate in the strict publication path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualityAdmission {
    contract_version: u16,
    scope_digest: Sha256Digest,
    comparison_identity_digest: Sha256Digest,
    raw_baseline_identity_digest: Sha256Digest,
    baseline_outcome_digest: Sha256Digest,
    baseline_receipt_digest: Sha256Digest,
    candidate_identity_digest: Option<Sha256Digest>,
    candidate_outcome_digest: Option<Sha256Digest>,
    pairing_method_digest: Sha256Digest,
    protected_predicate_digest: Sha256Digest,
    verifier_identity_digest: Sha256Digest,
    class_certificate_digest: Option<Sha256Digest>,
    confidence_scope_digest: Option<Sha256Digest>,
    evidence_class: QualityEvidenceClass,
    selection: QualitySelection,
    guarantee: QualityGuarantee,
    strict_improvement: bool,
    evidence_digest: Sha256Digest,
    admission_digest: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualityAdmissionRecord {
    pub contract_version: u16,
    pub scope_digest: Sha256Digest,
    pub comparison_identity_digest: Sha256Digest,
    pub raw_baseline_identity_digest: Sha256Digest,
    pub baseline_outcome_digest: Sha256Digest,
    pub baseline_receipt_digest: Sha256Digest,
    pub candidate_identity_digest: Option<Sha256Digest>,
    pub candidate_outcome_digest: Option<Sha256Digest>,
    pub pairing_method_digest: Sha256Digest,
    pub protected_predicate_digest: Sha256Digest,
    pub verifier_identity_digest: Sha256Digest,
    pub class_certificate_digest: Option<Sha256Digest>,
    pub confidence_scope_digest: Option<Sha256Digest>,
    pub evidence_class: QualityEvidenceClass,
    pub selection: QualitySelection,
    pub guarantee: QualityGuarantee,
    pub strict_improvement: bool,
    pub evidence_digest: Sha256Digest,
    pub admission_digest: Sha256Digest,
}

impl QualityAdmissionRecord {
    pub fn validate(&self) -> Result<(), QualityEnvelopeError> {
        QualityAdmission {
            contract_version: self.contract_version,
            scope_digest: self.scope_digest,
            comparison_identity_digest: self.comparison_identity_digest,
            raw_baseline_identity_digest: self.raw_baseline_identity_digest,
            baseline_outcome_digest: self.baseline_outcome_digest,
            baseline_receipt_digest: self.baseline_receipt_digest,
            candidate_identity_digest: self.candidate_identity_digest,
            candidate_outcome_digest: self.candidate_outcome_digest,
            pairing_method_digest: self.pairing_method_digest,
            protected_predicate_digest: self.protected_predicate_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            class_certificate_digest: self.class_certificate_digest,
            confidence_scope_digest: self.confidence_scope_digest,
            evidence_class: self.evidence_class,
            selection: self.selection,
            guarantee: self.guarantee,
            strict_improvement: self.strict_improvement,
            evidence_digest: self.evidence_digest,
            admission_digest: self.admission_digest,
        }
        .validate()
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeError> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, QualityEnvelopeError> {
        if bytes.len() > QUALITY_ENVELOPE_MAX_CANONICAL_BYTES {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::CanonicalPayloadTooLarge,
                "quality admission record exceeds the canonical byte bound",
            ));
        }
        let record: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        record.validate()?;
        if record.canonical_bytes()? != bytes {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::NonCanonicalEncoding,
                "quality admission record is not canonical sorted-key JSON",
            ));
        }
        Ok(record)
    }
}

impl QualityAdmission {
    pub fn admit_strict(
        evidence: QualityEvidence,
        baseline: FrozenBaseline,
    ) -> Result<Self, QualityEnvelopeError> {
        let (
            scope_digest,
            comparison_identity_digest,
            expected_baseline_identity,
            expected_baseline_outcome,
            candidate_identity_digest,
            candidate_outcome_digest,
            pairing_method_digest,
            protected_predicate_digest,
            verifier_identity_digest,
            class_certificate_digest,
            confidence_scope_digest,
            evidence_class,
            selection,
            guarantee,
            strict_improvement,
            evidence_digest,
        ) = match evidence {
            QualityEvidence::ExactNeutral(certificate) => {
                certificate.validate()?;
                (
                    certificate.task_digest,
                    certificate.comparison_identity_digest,
                    certificate.raw_baseline_identity_digest,
                    Some(certificate.protected_outcome_digest),
                    Some(certificate.candidate_identity_digest),
                    Some(certificate.protected_outcome_digest),
                    domain_digest(ADMISSION_DOMAIN, b"exact-continuation-pairing-v1"),
                    domain_digest(ADMISSION_DOMAIN, b"exact-protected-identity-v1"),
                    domain_digest(ADMISSION_DOMAIN, b"builtin-exact-verifier-v1"),
                    None,
                    None,
                    QualityEvidenceClass::ExactNeutral,
                    QualitySelection::Candidate,
                    QualityGuarantee::ExactSubstitution,
                    false,
                    certificate.certificate_digest,
                )
            }
            QualityEvidence::PointwiseDominance(certificate) => {
                certificate.validate()?;
                (
                    certificate.task_digest,
                    certificate.comparison_identity_digest,
                    certificate.raw_baseline_identity_digest,
                    Some(certificate.baseline_outcome_digest),
                    Some(certificate.candidate_identity_digest),
                    Some(certificate.candidate_outcome_digest),
                    certificate.pairing_method_digest,
                    certificate.protected_predicate_digest,
                    certificate.verifier_identity_digest,
                    None,
                    None,
                    QualityEvidenceClass::PointwiseDominance,
                    QualitySelection::Candidate,
                    QualityGuarantee::PointwiseNoWorse,
                    certificate.strictly_better,
                    certificate.certificate_digest,
                )
            }
            QualityEvidence::ScopedClassDominance(certificate) => {
                certificate.validate()?;
                (
                    certificate.task_digest,
                    certificate.comparison_identity_digest,
                    certificate.raw_baseline_identity_digest,
                    None,
                    Some(certificate.candidate_protocol_digest),
                    None,
                    certificate.membership_digest,
                    certificate.class_rule_digest,
                    digest_body(
                        SCOPED_DOMAIN,
                        json!({
                            "class_verifier_identity_digest": certificate.class_verifier_identity_digest,
                            "membership_verifier_identity_digest": certificate.membership_verifier_identity_digest,
                        }),
                    )?,
                    Some(certificate.certificate_digest),
                    None,
                    QualityEvidenceClass::ScopedClassDominance,
                    QualitySelection::Candidate,
                    QualityGuarantee::ScopedClassNoWorse,
                    certificate.claim == DominanceClaim::StrictlyBetter,
                    certificate.certificate_digest,
                )
            }
            QualityEvidence::Distributional(certificate) => {
                certificate.validate()?;
                (
                    certificate.benchmark_digest,
                    certificate.comparison_identity_digest,
                    certificate.raw_baseline_identity_digest,
                    Some(certificate.baseline_outcome_digest),
                    Some(certificate.candidate_protocol_digest),
                    None,
                    certificate.pairing_method_digest,
                    certificate.protected_predicate_digest,
                    certificate.verifier_identity_digest,
                    None,
                    Some(certificate.benchmark_digest),
                    QualityEvidenceClass::Distributional,
                    QualitySelection::FrozenBaseline,
                    QualityGuarantee::DistributionalOnly,
                    false,
                    certificate.certificate_digest,
                )
            }
            QualityEvidence::Unidentified {
                scope_digest,
                comparison_identity_digest,
                candidate_identity_digest,
                reason,
            } => (
                scope_digest,
                comparison_identity_digest,
                baseline.identity_digest,
                Some(baseline.protected_outcome_digest),
                Some(candidate_identity_digest),
                None,
                domain_digest(ADMISSION_DOMAIN, b"unidentified-pairing-v1"),
                domain_digest(ADMISSION_DOMAIN, b"unidentified-protected-predicate-v1"),
                domain_digest(ADMISSION_DOMAIN, b"builtin-fallback-verifier-v1"),
                None,
                None,
                QualityEvidenceClass::Unidentified,
                QualitySelection::FrozenBaseline,
                QualityGuarantee::Unidentified,
                false,
                domain_digest(
                    ADMISSION_DOMAIN,
                    canonical_json(&json!({
                        "candidate_identity_digest": candidate_identity_digest,
                        "comparison_identity_digest": comparison_identity_digest,
                        "reason": reason,
                        "scope_digest": scope_digest,
                    }))
                    .as_bytes(),
                ),
            ),
        };
        if expected_baseline_identity != baseline.identity_digest
            || expected_baseline_outcome
                .is_some_and(|digest| digest != baseline.protected_outcome_digest)
        {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::BaselineBindingMismatch,
                "quality evidence binds another frozen baseline identity or outcome",
            ));
        }
        let mut admission = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION,
            scope_digest,
            comparison_identity_digest,
            raw_baseline_identity_digest: baseline.identity_digest,
            baseline_outcome_digest: baseline.protected_outcome_digest,
            baseline_receipt_digest: baseline.receipt_digest,
            candidate_identity_digest,
            candidate_outcome_digest,
            pairing_method_digest,
            protected_predicate_digest,
            verifier_identity_digest,
            class_certificate_digest,
            confidence_scope_digest,
            evidence_class,
            selection,
            guarantee,
            strict_improvement,
            evidence_digest,
            admission_digest: Sha256Digest::ZERO,
        };
        admission.admission_digest = admission.expected_digest()?;
        admission.validate()?;
        Ok(admission)
    }

    pub fn validate(&self) -> Result<(), QualityEnvelopeError> {
        require_nonzero(
            "quality admission",
            &[
                self.scope_digest,
                self.comparison_identity_digest,
                self.raw_baseline_identity_digest,
                self.baseline_outcome_digest,
                self.baseline_receipt_digest,
                self.pairing_method_digest,
                self.protected_predicate_digest,
                self.verifier_identity_digest,
                self.evidence_digest,
            ],
        )?;
        if self.candidate_identity_digest.is_none()
            || self
                .candidate_identity_digest
                .is_some_and(|digest| digest == Sha256Digest::ZERO)
        {
            return Err(missing_binding("candidate identity"));
        }
        if self
            .candidate_outcome_digest
            .is_some_and(|digest| digest == Sha256Digest::ZERO)
            || self
                .class_certificate_digest
                .is_some_and(|digest| digest == Sha256Digest::ZERO)
            || self
                .confidence_scope_digest
                .is_some_and(|digest| digest == Sha256Digest::ZERO)
        {
            return Err(missing_binding("quality evidence detail"));
        }
        let coherent = matches!(
            (self.evidence_class, self.selection, self.guarantee),
            (
                QualityEvidenceClass::ExactNeutral,
                QualitySelection::Candidate,
                QualityGuarantee::ExactSubstitution
            ) | (
                QualityEvidenceClass::PointwiseDominance,
                QualitySelection::Candidate,
                QualityGuarantee::PointwiseNoWorse
            ) | (
                QualityEvidenceClass::ScopedClassDominance,
                QualitySelection::Candidate,
                QualityGuarantee::ScopedClassNoWorse
            ) | (
                QualityEvidenceClass::Distributional,
                QualitySelection::FrozenBaseline,
                QualityGuarantee::DistributionalOnly
            ) | (
                QualityEvidenceClass::Unidentified,
                QualitySelection::FrozenBaseline,
                QualityGuarantee::Unidentified
            )
        );
        let improvement_valid = !self.strict_improvement
            || matches!(
                self.evidence_class,
                QualityEvidenceClass::PointwiseDominance
                    | QualityEvidenceClass::ScopedClassDominance
            );
        let detail_valid = match self.evidence_class {
            QualityEvidenceClass::ExactNeutral => {
                self.candidate_outcome_digest == Some(self.baseline_outcome_digest)
                    && self.class_certificate_digest.is_none()
                    && self.confidence_scope_digest.is_none()
            }
            QualityEvidenceClass::PointwiseDominance => {
                self.candidate_outcome_digest.is_some()
                    && self.class_certificate_digest.is_none()
                    && self.confidence_scope_digest.is_none()
            }
            QualityEvidenceClass::ScopedClassDominance => {
                self.candidate_outcome_digest.is_none()
                    && self.class_certificate_digest == Some(self.evidence_digest)
                    && self.confidence_scope_digest.is_none()
            }
            QualityEvidenceClass::Distributional => {
                self.candidate_outcome_digest.is_none()
                    && self.class_certificate_digest.is_none()
                    && self.confidence_scope_digest == Some(self.scope_digest)
            }
            QualityEvidenceClass::Unidentified => {
                self.candidate_outcome_digest.is_none()
                    && self.class_certificate_digest.is_none()
                    && self.confidence_scope_digest.is_none()
            }
        };
        if !coherent || !improvement_valid || !detail_valid {
            return Err(QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::InvalidAdmission,
                "quality evidence class, selection, and guarantee are inconsistent",
            ));
        }
        require_version_and_digest(
            self.contract_version,
            self.admission_digest,
            self.expected_digest()?,
            "quality admission",
        )
    }

    fn expected_digest(&self) -> Result<Sha256Digest, QualityEnvelopeError> {
        digest_body(
            ADMISSION_DOMAIN,
            json!({
                "baseline_outcome_digest": self.baseline_outcome_digest,
                "baseline_receipt_digest": self.baseline_receipt_digest,
                "candidate_identity_digest": self.candidate_identity_digest,
                "candidate_outcome_digest": self.candidate_outcome_digest,
                "class_certificate_digest": self.class_certificate_digest,
                "comparison_identity_digest": self.comparison_identity_digest,
                "confidence_scope_digest": self.confidence_scope_digest,
                "contract_version": self.contract_version,
                "evidence_class": self.evidence_class,
                "evidence_digest": self.evidence_digest,
                "guarantee": self.guarantee,
                "pairing_method_digest": self.pairing_method_digest,
                "protected_predicate_digest": self.protected_predicate_digest,
                "raw_baseline_identity_digest": self.raw_baseline_identity_digest,
                "scope_digest": self.scope_digest,
                "selection": self.selection,
                "strict_improvement": self.strict_improvement,
                "verifier_identity_digest": self.verifier_identity_digest,
            }),
        )
    }

    pub fn record(&self) -> QualityAdmissionRecord {
        QualityAdmissionRecord {
            contract_version: self.contract_version,
            scope_digest: self.scope_digest,
            comparison_identity_digest: self.comparison_identity_digest,
            raw_baseline_identity_digest: self.raw_baseline_identity_digest,
            baseline_outcome_digest: self.baseline_outcome_digest,
            baseline_receipt_digest: self.baseline_receipt_digest,
            candidate_identity_digest: self.candidate_identity_digest,
            candidate_outcome_digest: self.candidate_outcome_digest,
            pairing_method_digest: self.pairing_method_digest,
            protected_predicate_digest: self.protected_predicate_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            class_certificate_digest: self.class_certificate_digest,
            confidence_scope_digest: self.confidence_scope_digest,
            evidence_class: self.evidence_class,
            selection: self.selection,
            guarantee: self.guarantee,
            strict_improvement: self.strict_improvement,
            evidence_digest: self.evidence_digest,
            admission_digest: self.admission_digest,
        }
    }

    pub const fn evidence_class(&self) -> QualityEvidenceClass {
        self.evidence_class
    }
    pub const fn selection(&self) -> QualitySelection {
        self.selection
    }
    pub const fn guarantee(&self) -> QualityGuarantee {
        self.guarantee
    }
    pub const fn scope_digest(&self) -> Sha256Digest {
        self.scope_digest
    }
    pub const fn comparison_identity_digest(&self) -> Sha256Digest {
        self.comparison_identity_digest
    }
    pub const fn raw_baseline_identity_digest(&self) -> Sha256Digest {
        self.raw_baseline_identity_digest
    }
    pub const fn baseline_outcome_digest(&self) -> Sha256Digest {
        self.baseline_outcome_digest
    }
    pub const fn baseline_receipt_digest(&self) -> Sha256Digest {
        self.baseline_receipt_digest
    }
    pub const fn candidate_identity_digest(&self) -> Option<Sha256Digest> {
        self.candidate_identity_digest
    }
    pub const fn candidate_outcome_digest(&self) -> Option<Sha256Digest> {
        self.candidate_outcome_digest
    }
    pub const fn pairing_method_digest(&self) -> Sha256Digest {
        self.pairing_method_digest
    }
    pub const fn protected_predicate_digest(&self) -> Sha256Digest {
        self.protected_predicate_digest
    }
    pub const fn verifier_identity_digest(&self) -> Sha256Digest {
        self.verifier_identity_digest
    }
    pub const fn class_certificate_digest(&self) -> Option<Sha256Digest> {
        self.class_certificate_digest
    }
    pub const fn confidence_scope_digest(&self) -> Option<Sha256Digest> {
        self.confidence_scope_digest
    }
    pub const fn strict_improvement(&self) -> bool {
        self.strict_improvement
    }
    pub const fn evidence_digest(&self) -> Sha256Digest {
        self.evidence_digest
    }
    pub const fn digest(&self) -> Sha256Digest {
        self.admission_digest
    }
}

pub fn quality_envelope_contract_manifest() -> Value {
    json!({
        "admission_classes": [
            "exact_neutral",
            "pointwise_dominance",
            "scoped_class_dominance",
            "distributional",
            "unidentified",
        ],
        "certificate_requirements": {
            "exact_neutral": ["candidate_identity", "continuation_identity", "model_visible_input", "protected_outcome"],
            "pointwise_dominance": ["same_task", "same_comparison_identity", "candidate_identity", "paired_outcomes", "pareto_vector", "pairing_method", "protected_predicate", "locked_verifier"],
            "scoped_class_dominance": ["candidate_protocol", "class_rule", "machine_checked_membership", "exact_verified_evidence_payloads", "locked_rule_and_membership_verifiers"],
            "distributional": ["frozen_benchmark", "paired_counts", "pairing_method", "protected_predicate", "positive_lower_bound_ppm", "locked_verifier"],
        },
        "contract_version": QUALITY_ENVELOPE_CONTRACT_VERSION,
        "distributional_arithmetic": "signed_integer_ppm",
        "distributional_strict_selection": "frozen_baseline",
        "linked_capabilities": ["zero_cert::VerifiedEvidence"],
        "max_canonical_bytes": QUALITY_ENVELOPE_MAX_CANONICAL_BYTES,
        "max_dimensions": QUALITY_ENVELOPE_MAX_DIMENSIONS,
        "name": "zerostack.quality_envelope.v1",
        "negative_space": [
            "float_quality_arithmetic",
            "heuristic_quality_evidence",
            "pointwise_claim_from_distributional_evidence",
            "quality_inference_from_tokens_or_cache_hits",
            "unlabeled_population_percentage_claim",
        ],
        "paired_mean_rounding": "signed_integer_division_toward_zero",
        "performance_receipt_bindings": [
            "comparison_identity",
            "candidate_identity",
            "raw_baseline_identity",
            "pairing_method",
            "protected_predicate",
            "verifier_identity",
            "candidate_outcome_or_class_certificate",
            "baseline_outcome",
            "admission_selection",
            "strict_improvement",
            "confidence_scope",
            "evidence_digest",
        ],
        "protected_order": "declared_pareto_vector",
        "scoped_proof_carrier": "zero_cert_verified_exact_rule_and_membership_payloads_with_locked_provenance",
        "strict_candidate_classes": [
            "exact_neutral",
            "pointwise_dominance",
            "scoped_class_dominance",
        ],
    })
}

pub fn quality_envelope_contract_digest() -> Sha256Digest {
    domain_digest(
        CONTRACT_DOMAIN,
        canonical_json(&quality_envelope_contract_manifest()).as_bytes(),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QualityEnvelopeFailureCode {
    SchemaVersionMismatch,
    MissingBinding,
    InvalidProtectedVector,
    CandidateRegression,
    NonCanonicalOrder,
    NonCanonicalEncoding,
    CanonicalPayloadTooLarge,
    ExactNeutralMismatch,
    EvidencePayloadMismatch,
    EvidenceInvalid,
    CertificateDigestMismatch,
    ClassMembershipMismatch,
    InvalidDistributionalCounts,
    InvalidDistributionalBound,
    NonPositiveDistributionalBound,
    BaselineBindingMismatch,
    InvalidAdmission,
    SerializationFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QualityEnvelopeError {
    code: QualityEnvelopeFailureCode,
    detail: String,
}

impl QualityEnvelopeError {
    fn new(code: QualityEnvelopeFailureCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> QualityEnvelopeFailureCode {
        self.code
    }
}

impl fmt::Display for QualityEnvelopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.detail)
    }
}

impl Error for QualityEnvelopeError {}

fn validate_id(field: &str, value: &str) -> Result<(), QualityEnvelopeError> {
    if value.is_empty()
        || value.len() > QUALITY_ENVELOPE_MAX_METRIC_ID_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(QualityEnvelopeError::new(
            QualityEnvelopeFailureCode::InvalidProtectedVector,
            format!("{field} is empty, contains control characters, or exceeds its bound"),
        ));
    }
    Ok(())
}

fn require_nonzero(label: &str, digests: &[Sha256Digest]) -> Result<(), QualityEnvelopeError> {
    if digests.contains(&Sha256Digest::ZERO) {
        Err(QualityEnvelopeError::new(
            QualityEnvelopeFailureCode::MissingBinding,
            format!("{label} contains a zero digest"),
        ))
    } else {
        Ok(())
    }
}

fn missing_binding(label: &str) -> QualityEnvelopeError {
    QualityEnvelopeError::new(
        QualityEnvelopeFailureCode::MissingBinding,
        format!("{label} digest is zero"),
    )
}

fn version_error(label: &str) -> QualityEnvelopeError {
    QualityEnvelopeError::new(
        QualityEnvelopeFailureCode::SchemaVersionMismatch,
        format!("{label} contract version is not current"),
    )
}

fn require_version_and_digest(
    version: u16,
    actual: Sha256Digest,
    expected: Sha256Digest,
    label: &str,
) -> Result<(), QualityEnvelopeError> {
    if version != QUALITY_ENVELOPE_CONTRACT_VERSION {
        return Err(version_error(label));
    }
    if actual == Sha256Digest::ZERO || actual != expected {
        return Err(QualityEnvelopeError::new(
            QualityEnvelopeFailureCode::CertificateDigestMismatch,
            format!("{label} digest does not match its canonical body"),
        ));
    }
    Ok(())
}

fn require_exact_payload(
    label: &str,
    expected: &[u8],
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<(), QualityEnvelopeError> {
    if evidence.certificate().payload.as_ref() != expected {
        return Err(QualityEnvelopeError::new(
            QualityEnvelopeFailureCode::EvidencePayloadMismatch,
            format!("verified evidence payload does not equal canonical {label} bytes"),
        ));
    }
    Ok(())
}

fn evidence_digest(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<Sha256Digest, QualityEnvelopeError> {
    evidence
        .certificate()
        .canonical_digest()
        .map(Sha256Digest::from_bytes)
        .map_err(|error| {
            QualityEnvelopeError::new(
                QualityEnvelopeFailureCode::EvidenceInvalid,
                error.to_string(),
            )
        })
}

fn verifier_identity_digest(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<Sha256Digest, QualityEnvelopeError> {
    let provenance = &evidence.certificate().provenance;
    digest_body(
        VERIFIER_DOMAIN,
        json!({
            "index_id": provenance.index_id,
            "index_version": provenance.index_version,
            "operator_id": provenance.operator_id,
            "operator_version": provenance.operator_version,
            "parser_id": provenance.parser_id,
            "parser_version": provenance.parser_version,
        }),
    )
}

fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, QualityEnvelopeError> {
    let value = serde_json::to_value(value).map_err(json_error)?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() > QUALITY_ENVELOPE_MAX_CANONICAL_BYTES {
        return Err(QualityEnvelopeError::new(
            QualityEnvelopeFailureCode::CanonicalPayloadTooLarge,
            "canonical quality payload exceeds its byte bound",
        ));
    }
    Ok(bytes)
}

fn digest_body(domain: &[u8], value: Value) -> Result<Sha256Digest, QualityEnvelopeError> {
    Ok(domain_digest(domain, canonical_json(&value).as_bytes()))
}

fn domain_digest(domain: &[u8], payload: &[u8]) -> Sha256Digest {
    let mut bytes = Vec::with_capacity(domain.len() + payload.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(payload);
    Sha256Digest::from_bytes(sha256(&bytes))
}

fn json_error(error: serde_json::Error) -> QualityEnvelopeError {
    QualityEnvelopeError::new(
        QualityEnvelopeFailureCode::SerializationFailure,
        error.to_string(),
    )
}

