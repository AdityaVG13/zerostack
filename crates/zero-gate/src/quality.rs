//! Proof-carrying protected-quality envelope for strict candidate admission.
//!
//! The envelope keeps exact, pointwise, scoped-class, and distributional evidence
//! distinct. Strict publication admits only evidence that protects the current
//! task pointwise. Distributional and unidentified candidates select the frozen
//! raw baseline instead of laundering a population claim into an individual one.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zero_abi::{DigestV1, canonical_json, sha256};
use zero_cert::VerifiedEvidence;

pub const QUALITY_ENVELOPE_CONTRACT_VERSION_V1: u16 = 1;
pub const QUALITY_ENVELOPE_MAX_CANONICAL_BYTES_V1: usize = 1_048_576;
pub const QUALITY_ENVELOPE_MAX_DIMENSIONS_V1: usize = 128;
pub const QUALITY_ENVELOPE_MAX_METRIC_ID_BYTES_V1: usize = 128;
pub const QUALITY_PPM_SCALE_V1: i64 = 1_000_000;

const PAIR_DOMAIN_V1: &[u8] = b"zerostack.quality.pair.v1\0";
const EXACT_DOMAIN_V1: &[u8] = b"zerostack.quality.exact_neutral.v1\0";
const POINTWISE_DOMAIN_V1: &[u8] = b"zerostack.quality.pointwise.v1\0";
const CLASS_RULE_DOMAIN_V1: &[u8] = b"zerostack.quality.class_rule.v1\0";
const MEMBERSHIP_DOMAIN_V1: &[u8] = b"zerostack.quality.membership.v1\0";
const SCOPED_DOMAIN_V1: &[u8] = b"zerostack.quality.scoped.v1\0";
const DISTRIBUTIONAL_CLAIM_DOMAIN_V1: &[u8] = b"zerostack.quality.distributional_claim.v1\0";
const DISTRIBUTIONAL_DOMAIN_V1: &[u8] = b"zerostack.quality.distributional.v1\0";
const VERIFIER_DOMAIN_V1: &[u8] = b"zerostack.quality.verifier.v1\0";
const ADMISSION_DOMAIN_V1: &[u8] = b"zerostack.quality.admission.v1\0";
const CONTRACT_DOMAIN_V1: &[u8] = b"zerostack.quality.contract.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricOrderV1 {
    AtLeast,
    AtMost,
    Exact,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedMetricV1 {
    pub metric_id: String,
    pub order: MetricOrderV1,
    pub baseline_value: i64,
    pub candidate_value: i64,
}

impl ProtectedMetricV1 {
    fn validate(&self) -> Result<(), QualityEnvelopeErrorV1> {
        validate_id("metric_id", &self.metric_id)?;
        if self.no_worse() {
            Ok(())
        } else {
            Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::CandidateRegression,
                format!("candidate regresses protected metric {}", self.metric_id),
            ))
        }
    }

    pub const fn no_worse(&self) -> bool {
        match self.order {
            MetricOrderV1::AtLeast => self.candidate_value >= self.baseline_value,
            MetricOrderV1::AtMost => self.candidate_value <= self.baseline_value,
            MetricOrderV1::Exact => self.candidate_value == self.baseline_value,
        }
    }

    pub const fn strictly_better(&self) -> bool {
        match self.order {
            MetricOrderV1::AtLeast => self.candidate_value > self.baseline_value,
            MetricOrderV1::AtMost => self.candidate_value < self.baseline_value,
            MetricOrderV1::Exact => false,
        }
    }
}

/// Canonical paired protected outcomes for one task and comparison identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualityPairV1 {
    contract_version: u16,
    task_digest: DigestV1,
    comparison_identity_digest: DigestV1,
    raw_baseline_identity_digest: DigestV1,
    candidate_identity_digest: DigestV1,
    baseline_outcome_digest: DigestV1,
    candidate_outcome_digest: DigestV1,
    protected_schema_digest: DigestV1,
    pairing_method_digest: DigestV1,
    dimensions: Vec<ProtectedMetricV1>,
}

impl QualityPairV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_digest: DigestV1,
        comparison_identity_digest: DigestV1,
        raw_baseline_identity_digest: DigestV1,
        candidate_identity_digest: DigestV1,
        baseline_outcome_digest: DigestV1,
        candidate_outcome_digest: DigestV1,
        protected_schema_digest: DigestV1,
        pairing_method_digest: DigestV1,
        dimensions: Vec<ProtectedMetricV1>,
    ) -> Result<Self, QualityEnvelopeErrorV1> {
        let pair = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION_V1,
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

    pub fn validate(&self) -> Result<(), QualityEnvelopeErrorV1> {
        if self.contract_version != QUALITY_ENVELOPE_CONTRACT_VERSION_V1 {
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::SchemaVersionMismatch,
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
        if self.dimensions.is_empty() || self.dimensions.len() > QUALITY_ENVELOPE_MAX_DIMENSIONS_V1
        {
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::InvalidProtectedVector,
                "protected vector is empty or exceeds its bound",
            ));
        }
        let mut previous: Option<&str> = None;
        for metric in &self.dimensions {
            metric.validate()?;
            if previous.is_some_and(|value| value >= metric.metric_id.as_str()) {
                return Err(QualityEnvelopeErrorV1::new(
                    QualityEnvelopeFailureCodeV1::NonCanonicalOrder,
                    "protected metric ids must be unique and strictly sorted",
                ));
            }
            previous = Some(&metric.metric_id);
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, QualityEnvelopeErrorV1> {
        if bytes.len() > QUALITY_ENVELOPE_MAX_CANONICAL_BYTES_V1 {
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::CanonicalPayloadTooLarge,
                "quality pair exceeds the canonical byte bound",
            ));
        }
        let pair: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        pair.validate()?;
        if pair.canonical_bytes()? != bytes {
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::NonCanonicalEncoding,
                "quality pair bytes are not canonical sorted-key JSON",
            ));
        }
        Ok(pair)
    }

    pub fn digest(&self) -> Result<DigestV1, QualityEnvelopeErrorV1> {
        Ok(domain_digest(PAIR_DOMAIN_V1, &self.canonical_bytes()?))
    }

    pub const fn task_digest(&self) -> DigestV1 {
        self.task_digest
    }
    pub const fn comparison_identity_digest(&self) -> DigestV1 {
        self.comparison_identity_digest
    }
    pub const fn raw_baseline_identity_digest(&self) -> DigestV1 {
        self.raw_baseline_identity_digest
    }
    pub const fn baseline_outcome_digest(&self) -> DigestV1 {
        self.baseline_outcome_digest
    }
    pub const fn candidate_outcome_digest(&self) -> DigestV1 {
        self.candidate_outcome_digest
    }
    pub fn strictly_better(&self) -> bool {
        self.dimensions
            .iter()
            .any(ProtectedMetricV1::strictly_better)
    }
}

/// Equality-by-substitution certificate. The constructor requires both sides
/// of every protected continuation identity to match exactly.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExactNeutralCertificateV1 {
    contract_version: u16,
    task_digest: DigestV1,
    comparison_identity_digest: DigestV1,
    raw_baseline_identity_digest: DigestV1,
    candidate_identity_digest: DigestV1,
    continuation_identity_digest: DigestV1,
    model_visible_input_digest: DigestV1,
    protected_outcome_digest: DigestV1,
    certificate_digest: DigestV1,
}

impl ExactNeutralCertificateV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn verify(
        task_digest: DigestV1,
        comparison_identity_digest: DigestV1,
        raw_baseline_identity_digest: DigestV1,
        candidate_identity_digest: DigestV1,
        baseline_continuation_identity_digest: DigestV1,
        candidate_continuation_identity_digest: DigestV1,
        baseline_model_visible_input_digest: DigestV1,
        candidate_model_visible_input_digest: DigestV1,
        baseline_protected_outcome_digest: DigestV1,
        candidate_protected_outcome_digest: DigestV1,
    ) -> Result<Self, QualityEnvelopeErrorV1> {
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
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::ExactNeutralMismatch,
                "continuation, model-visible input, and protected outcome must all match",
            ));
        }
        let mut certificate = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION_V1,
            task_digest,
            comparison_identity_digest,
            raw_baseline_identity_digest,
            candidate_identity_digest,
            continuation_identity_digest: baseline_continuation_identity_digest,
            model_visible_input_digest: baseline_model_visible_input_digest,
            protected_outcome_digest: baseline_protected_outcome_digest,
            certificate_digest: DigestV1::ZERO,
        };
        certificate.certificate_digest = certificate.expected_digest()?;
        Ok(certificate)
    }

    pub fn validate(&self) -> Result<(), QualityEnvelopeErrorV1> {
        require_version_and_digest(
            self.contract_version,
            self.certificate_digest,
            self.expected_digest()?,
            "exact-neutral certificate",
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    fn expected_digest(&self) -> Result<DigestV1, QualityEnvelopeErrorV1> {
        digest_body(
            EXACT_DOMAIN_V1,
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
pub struct PointwiseDominanceCertificateV1 {
    contract_version: u16,
    task_digest: DigestV1,
    comparison_identity_digest: DigestV1,
    raw_baseline_identity_digest: DigestV1,
    candidate_identity_digest: DigestV1,
    baseline_outcome_digest: DigestV1,
    candidate_outcome_digest: DigestV1,
    pair_digest: DigestV1,
    pairing_method_digest: DigestV1,
    protected_predicate_digest: DigestV1,
    evidence_digest: DigestV1,
    verifier_identity_digest: DigestV1,
    strictly_better: bool,
    certificate_digest: DigestV1,
}

impl PointwiseDominanceCertificateV1 {
    pub fn verify(
        pair: &QualityPairV1,
        protected_predicate_digest: DigestV1,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, QualityEnvelopeErrorV1> {
        pair.validate()?;
        if protected_predicate_digest == DigestV1::ZERO {
            return Err(missing_binding("protected predicate"));
        }
        let pair_bytes = pair.canonical_bytes()?;
        require_exact_payload("pointwise pair", &pair_bytes, evidence)?;
        let mut certificate = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION_V1,
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
            certificate_digest: DigestV1::ZERO,
        };
        certificate.certificate_digest = certificate.expected_digest()?;
        Ok(certificate)
    }

    pub fn validate(&self) -> Result<(), QualityEnvelopeErrorV1> {
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

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    fn expected_digest(&self) -> Result<DigestV1, QualityEnvelopeErrorV1> {
        digest_body(
            POINTWISE_DOMAIN_V1,
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
pub enum DominanceClaimV1 {
    NoWorse,
    StrictlyBetter,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClassDominanceRuleV1 {
    contract_version: u16,
    class_digest: DigestV1,
    comparison_identity_digest: DigestV1,
    protected_schema_digest: DigestV1,
    candidate_protocol_digest: DigestV1,
    raw_baseline_identity_digest: DigestV1,
    dominance_rule_digest: DigestV1,
    claim: DominanceClaimV1,
}

impl ClassDominanceRuleV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        class_digest: DigestV1,
        comparison_identity_digest: DigestV1,
        protected_schema_digest: DigestV1,
        candidate_protocol_digest: DigestV1,
        raw_baseline_identity_digest: DigestV1,
        dominance_rule_digest: DigestV1,
        claim: DominanceClaimV1,
    ) -> Result<Self, QualityEnvelopeErrorV1> {
        let rule = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION_V1,
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

    fn validate(&self) -> Result<(), QualityEnvelopeErrorV1> {
        if self.contract_version != QUALITY_ENVELOPE_CONTRACT_VERSION_V1 {
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

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<DigestV1, QualityEnvelopeErrorV1> {
        Ok(domain_digest(
            CLASS_RULE_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskClassMembershipV1 {
    contract_version: u16,
    class_digest: DigestV1,
    task_digest: DigestV1,
    candidate_protocol_digest: DigestV1,
    membership_predicate_digest: DigestV1,
}

impl TaskClassMembershipV1 {
    pub fn new(
        class_digest: DigestV1,
        task_digest: DigestV1,
        candidate_protocol_digest: DigestV1,
        membership_predicate_digest: DigestV1,
    ) -> Result<Self, QualityEnvelopeErrorV1> {
        let membership = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION_V1,
            class_digest,
            task_digest,
            candidate_protocol_digest,
            membership_predicate_digest,
        };
        membership.validate()?;
        Ok(membership)
    }

    fn validate(&self) -> Result<(), QualityEnvelopeErrorV1> {
        if self.contract_version != QUALITY_ENVELOPE_CONTRACT_VERSION_V1 {
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

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<DigestV1, QualityEnvelopeErrorV1> {
        Ok(domain_digest(
            MEMBERSHIP_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

/// Opaque reusable class proof plus exact task-membership proof.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedClassDominanceCertificateV1 {
    contract_version: u16,
    class_digest: DigestV1,
    task_digest: DigestV1,
    comparison_identity_digest: DigestV1,
    raw_baseline_identity_digest: DigestV1,
    candidate_protocol_digest: DigestV1,
    class_rule_digest: DigestV1,
    membership_digest: DigestV1,
    class_evidence_digest: DigestV1,
    membership_evidence_digest: DigestV1,
    class_verifier_identity_digest: DigestV1,
    membership_verifier_identity_digest: DigestV1,
    claim: DominanceClaimV1,
    certificate_digest: DigestV1,
}

impl ScopedClassDominanceCertificateV1 {
    pub fn verify(
        rule: &ClassDominanceRuleV1,
        membership: &TaskClassMembershipV1,
        class_evidence: &VerifiedEvidence<'_, '_>,
        membership_evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, QualityEnvelopeErrorV1> {
        rule.validate()?;
        membership.validate()?;
        if rule.class_digest != membership.class_digest
            || rule.candidate_protocol_digest != membership.candidate_protocol_digest
        {
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::ClassMembershipMismatch,
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
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION_V1,
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
            certificate_digest: DigestV1::ZERO,
        };
        certificate.certificate_digest = certificate.expected_digest()?;
        Ok(certificate)
    }

    pub fn validate(&self) -> Result<(), QualityEnvelopeErrorV1> {
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

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    fn expected_digest(&self) -> Result<DigestV1, QualityEnvelopeErrorV1> {
        digest_body(
            SCOPED_DOMAIN_V1,
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
pub struct DistributionalClaimV1 {
    contract_version: u16,
    benchmark_digest: DigestV1,
    comparison_identity_digest: DigestV1,
    candidate_protocol_digest: DigestV1,
    raw_baseline_identity_digest: DigestV1,
    baseline_outcome_digest: DigestV1,
    protected_schema_digest: DigestV1,
    pairing_method_digest: DigestV1,
    protected_predicate_digest: DigestV1,
    paired_tasks: u64,
    candidate_wins: u64,
    protected_losses: u64,
    ties: u64,
    mean_gain_ppm: i64,
    lower_confidence_gain_ppm: i64,
    confidence_ppm: u32,
}

impl DistributionalClaimV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        benchmark_digest: DigestV1,
        comparison_identity_digest: DigestV1,
        candidate_protocol_digest: DigestV1,
        raw_baseline_identity_digest: DigestV1,
        baseline_outcome_digest: DigestV1,
        protected_schema_digest: DigestV1,
        pairing_method_digest: DigestV1,
        protected_predicate_digest: DigestV1,
        paired_tasks: u64,
        candidate_wins: u64,
        protected_losses: u64,
        ties: u64,
        mean_gain_ppm: i64,
        lower_confidence_gain_ppm: i64,
        confidence_ppm: u32,
    ) -> Result<Self, QualityEnvelopeErrorV1> {
        let claim = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION_V1,
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

    fn validate(&self) -> Result<(), QualityEnvelopeErrorV1> {
        if self.contract_version != QUALITY_ENVELOPE_CONTRACT_VERSION_V1 {
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
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::InvalidDistributionalCounts,
                "paired task count must equal wins plus protected losses plus ties",
            ));
        }
        if self.confidence_ppm == 0
            || i64::from(self.confidence_ppm) >= QUALITY_PPM_SCALE_V1
            || !(-QUALITY_PPM_SCALE_V1..=QUALITY_PPM_SCALE_V1).contains(&self.mean_gain_ppm)
            || !(-QUALITY_PPM_SCALE_V1..=QUALITY_PPM_SCALE_V1)
                .contains(&self.lower_confidence_gain_ppm)
            || self.lower_confidence_gain_ppm > self.mean_gain_ppm
        {
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::InvalidDistributionalBound,
                "confidence and paired gain ppm values are outside their frozen bounds",
            ));
        }
        let paired_delta = i128::from(self.candidate_wins) - i128::from(self.protected_losses);
        let expected_mean_ppm =
            paired_delta * i128::from(QUALITY_PPM_SCALE_V1) / i128::from(self.paired_tasks);
        if i128::from(self.mean_gain_ppm) != expected_mean_ppm {
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::InvalidDistributionalBound,
                "mean gain ppm does not equal the frozen paired win-loss calculation",
            ));
        }
        if self.lower_confidence_gain_ppm <= 0 {
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::NonPositiveDistributionalBound,
                "distributional admission requires a positive lower confidence gain",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<DigestV1, QualityEnvelopeErrorV1> {
        Ok(domain_digest(
            DISTRIBUTIONAL_CLAIM_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DistributionalCertificateV1 {
    contract_version: u16,
    benchmark_digest: DigestV1,
    comparison_identity_digest: DigestV1,
    candidate_protocol_digest: DigestV1,
    raw_baseline_identity_digest: DigestV1,
    baseline_outcome_digest: DigestV1,
    pairing_method_digest: DigestV1,
    protected_predicate_digest: DigestV1,
    claim_digest: DigestV1,
    paired_tasks: u64,
    protected_losses: u64,
    lower_confidence_gain_ppm: i64,
    confidence_ppm: u32,
    evidence_digest: DigestV1,
    verifier_identity_digest: DigestV1,
    certificate_digest: DigestV1,
}

impl DistributionalCertificateV1 {
    pub fn verify(
        claim: &DistributionalClaimV1,
        evidence: &VerifiedEvidence<'_, '_>,
    ) -> Result<Self, QualityEnvelopeErrorV1> {
        claim.validate()?;
        require_exact_payload("distributional claim", &claim.canonical_bytes()?, evidence)?;
        let mut certificate = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION_V1,
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
            certificate_digest: DigestV1::ZERO,
        };
        certificate.certificate_digest = certificate.expected_digest()?;
        Ok(certificate)
    }

    pub fn validate(&self) -> Result<(), QualityEnvelopeErrorV1> {
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
            || i64::from(self.confidence_ppm) >= QUALITY_PPM_SCALE_V1
            || self.lower_confidence_gain_ppm <= 0
        {
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::InvalidDistributionalBound,
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

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    fn expected_digest(&self) -> Result<DigestV1, QualityEnvelopeErrorV1> {
        digest_body(
            DISTRIBUTIONAL_DOMAIN_V1,
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
pub enum UnidentifiedReasonV1 {
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
pub enum QualityEvidenceV1 {
    ExactNeutral(ExactNeutralCertificateV1),
    PointwiseDominance(PointwiseDominanceCertificateV1),
    ScopedClassDominance(ScopedClassDominanceCertificateV1),
    Distributional(DistributionalCertificateV1),
    Unidentified {
        scope_digest: DigestV1,
        comparison_identity_digest: DigestV1,
        candidate_identity_digest: DigestV1,
        reason: UnidentifiedReasonV1,
    },
}

impl QualityEvidenceV1 {
    pub fn unidentified(
        scope_digest: DigestV1,
        comparison_identity_digest: DigestV1,
        candidate_identity_digest: DigestV1,
        reason: UnidentifiedReasonV1,
    ) -> Result<Self, QualityEnvelopeErrorV1> {
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
pub enum QualityEvidenceClassV1 {
    ExactNeutral,
    PointwiseDominance,
    ScopedClassDominance,
    Distributional,
    Unidentified,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualitySelectionV1 {
    Candidate,
    FrozenBaseline,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityGuaranteeV1 {
    ExactSubstitution,
    PointwiseNoWorse,
    ScopedClassNoWorse,
    DistributionalOnly,
    Unidentified,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenBaselineV1 {
    identity_digest: DigestV1,
    protected_outcome_digest: DigestV1,
    receipt_digest: DigestV1,
}

impl FrozenBaselineV1 {
    pub fn new(
        identity_digest: DigestV1,
        protected_outcome_digest: DigestV1,
        receipt_digest: DigestV1,
    ) -> Result<Self, QualityEnvelopeErrorV1> {
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

    pub const fn identity_digest(&self) -> DigestV1 {
        self.identity_digest
    }
    pub const fn protected_outcome_digest(&self) -> DigestV1 {
        self.protected_outcome_digest
    }
    pub const fn receipt_digest(&self) -> DigestV1 {
        self.receipt_digest
    }
}

/// Opaque G7 decision. Construction enforces that population-only evidence
/// cannot authorize an individual candidate in the strict publication path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualityAdmissionV1 {
    contract_version: u16,
    scope_digest: DigestV1,
    comparison_identity_digest: DigestV1,
    raw_baseline_identity_digest: DigestV1,
    baseline_outcome_digest: DigestV1,
    baseline_receipt_digest: DigestV1,
    candidate_identity_digest: Option<DigestV1>,
    candidate_outcome_digest: Option<DigestV1>,
    pairing_method_digest: DigestV1,
    protected_predicate_digest: DigestV1,
    verifier_identity_digest: DigestV1,
    class_certificate_digest: Option<DigestV1>,
    confidence_scope_digest: Option<DigestV1>,
    evidence_class: QualityEvidenceClassV1,
    selection: QualitySelectionV1,
    guarantee: QualityGuaranteeV1,
    strict_improvement: bool,
    evidence_digest: DigestV1,
    admission_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualityAdmissionRecordV1 {
    pub contract_version: u16,
    pub scope_digest: DigestV1,
    pub comparison_identity_digest: DigestV1,
    pub raw_baseline_identity_digest: DigestV1,
    pub baseline_outcome_digest: DigestV1,
    pub baseline_receipt_digest: DigestV1,
    pub candidate_identity_digest: Option<DigestV1>,
    pub candidate_outcome_digest: Option<DigestV1>,
    pub pairing_method_digest: DigestV1,
    pub protected_predicate_digest: DigestV1,
    pub verifier_identity_digest: DigestV1,
    pub class_certificate_digest: Option<DigestV1>,
    pub confidence_scope_digest: Option<DigestV1>,
    pub evidence_class: QualityEvidenceClassV1,
    pub selection: QualitySelectionV1,
    pub guarantee: QualityGuaranteeV1,
    pub strict_improvement: bool,
    pub evidence_digest: DigestV1,
    pub admission_digest: DigestV1,
}

impl QualityAdmissionRecordV1 {
    pub fn validate(&self) -> Result<(), QualityEnvelopeErrorV1> {
        QualityAdmissionV1 {
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

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, QualityEnvelopeErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, QualityEnvelopeErrorV1> {
        if bytes.len() > QUALITY_ENVELOPE_MAX_CANONICAL_BYTES_V1 {
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::CanonicalPayloadTooLarge,
                "quality admission record exceeds the canonical byte bound",
            ));
        }
        let record: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        record.validate()?;
        if record.canonical_bytes()? != bytes {
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::NonCanonicalEncoding,
                "quality admission record is not canonical sorted-key JSON",
            ));
        }
        Ok(record)
    }
}

impl QualityAdmissionV1 {
    pub fn admit_strict(
        evidence: QualityEvidenceV1,
        baseline: FrozenBaselineV1,
    ) -> Result<Self, QualityEnvelopeErrorV1> {
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
            QualityEvidenceV1::ExactNeutral(certificate) => {
                certificate.validate()?;
                (
                    certificate.task_digest,
                    certificate.comparison_identity_digest,
                    certificate.raw_baseline_identity_digest,
                    Some(certificate.protected_outcome_digest),
                    Some(certificate.candidate_identity_digest),
                    Some(certificate.protected_outcome_digest),
                    domain_digest(ADMISSION_DOMAIN_V1, b"exact-continuation-pairing-v1"),
                    domain_digest(ADMISSION_DOMAIN_V1, b"exact-protected-identity-v1"),
                    domain_digest(ADMISSION_DOMAIN_V1, b"builtin-exact-verifier-v1"),
                    None,
                    None,
                    QualityEvidenceClassV1::ExactNeutral,
                    QualitySelectionV1::Candidate,
                    QualityGuaranteeV1::ExactSubstitution,
                    false,
                    certificate.certificate_digest,
                )
            }
            QualityEvidenceV1::PointwiseDominance(certificate) => {
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
                    QualityEvidenceClassV1::PointwiseDominance,
                    QualitySelectionV1::Candidate,
                    QualityGuaranteeV1::PointwiseNoWorse,
                    certificate.strictly_better,
                    certificate.certificate_digest,
                )
            }
            QualityEvidenceV1::ScopedClassDominance(certificate) => {
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
                        SCOPED_DOMAIN_V1,
                        json!({
                            "class_verifier_identity_digest": certificate.class_verifier_identity_digest,
                            "membership_verifier_identity_digest": certificate.membership_verifier_identity_digest,
                        }),
                    )?,
                    Some(certificate.certificate_digest),
                    None,
                    QualityEvidenceClassV1::ScopedClassDominance,
                    QualitySelectionV1::Candidate,
                    QualityGuaranteeV1::ScopedClassNoWorse,
                    certificate.claim == DominanceClaimV1::StrictlyBetter,
                    certificate.certificate_digest,
                )
            }
            QualityEvidenceV1::Distributional(certificate) => {
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
                    QualityEvidenceClassV1::Distributional,
                    QualitySelectionV1::FrozenBaseline,
                    QualityGuaranteeV1::DistributionalOnly,
                    false,
                    certificate.certificate_digest,
                )
            }
            QualityEvidenceV1::Unidentified {
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
                domain_digest(ADMISSION_DOMAIN_V1, b"unidentified-pairing-v1"),
                domain_digest(ADMISSION_DOMAIN_V1, b"unidentified-protected-predicate-v1"),
                domain_digest(ADMISSION_DOMAIN_V1, b"builtin-fallback-verifier-v1"),
                None,
                None,
                QualityEvidenceClassV1::Unidentified,
                QualitySelectionV1::FrozenBaseline,
                QualityGuaranteeV1::Unidentified,
                false,
                domain_digest(
                    ADMISSION_DOMAIN_V1,
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
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::BaselineBindingMismatch,
                "quality evidence binds another frozen baseline identity or outcome",
            ));
        }
        let mut admission = Self {
            contract_version: QUALITY_ENVELOPE_CONTRACT_VERSION_V1,
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
            admission_digest: DigestV1::ZERO,
        };
        admission.admission_digest = admission.expected_digest()?;
        admission.validate()?;
        Ok(admission)
    }

    pub fn validate(&self) -> Result<(), QualityEnvelopeErrorV1> {
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
                .is_some_and(|digest| digest == DigestV1::ZERO)
        {
            return Err(missing_binding("candidate identity"));
        }
        if self
            .candidate_outcome_digest
            .is_some_and(|digest| digest == DigestV1::ZERO)
            || self
                .class_certificate_digest
                .is_some_and(|digest| digest == DigestV1::ZERO)
            || self
                .confidence_scope_digest
                .is_some_and(|digest| digest == DigestV1::ZERO)
        {
            return Err(missing_binding("quality evidence detail"));
        }
        let coherent = matches!(
            (self.evidence_class, self.selection, self.guarantee),
            (
                QualityEvidenceClassV1::ExactNeutral,
                QualitySelectionV1::Candidate,
                QualityGuaranteeV1::ExactSubstitution
            ) | (
                QualityEvidenceClassV1::PointwiseDominance,
                QualitySelectionV1::Candidate,
                QualityGuaranteeV1::PointwiseNoWorse
            ) | (
                QualityEvidenceClassV1::ScopedClassDominance,
                QualitySelectionV1::Candidate,
                QualityGuaranteeV1::ScopedClassNoWorse
            ) | (
                QualityEvidenceClassV1::Distributional,
                QualitySelectionV1::FrozenBaseline,
                QualityGuaranteeV1::DistributionalOnly
            ) | (
                QualityEvidenceClassV1::Unidentified,
                QualitySelectionV1::FrozenBaseline,
                QualityGuaranteeV1::Unidentified
            )
        );
        let improvement_valid = !self.strict_improvement
            || matches!(
                self.evidence_class,
                QualityEvidenceClassV1::PointwiseDominance
                    | QualityEvidenceClassV1::ScopedClassDominance
            );
        let detail_valid = match self.evidence_class {
            QualityEvidenceClassV1::ExactNeutral => {
                self.candidate_outcome_digest == Some(self.baseline_outcome_digest)
                    && self.class_certificate_digest.is_none()
                    && self.confidence_scope_digest.is_none()
            }
            QualityEvidenceClassV1::PointwiseDominance => {
                self.candidate_outcome_digest.is_some()
                    && self.class_certificate_digest.is_none()
                    && self.confidence_scope_digest.is_none()
            }
            QualityEvidenceClassV1::ScopedClassDominance => {
                self.candidate_outcome_digest.is_none()
                    && self.class_certificate_digest == Some(self.evidence_digest)
                    && self.confidence_scope_digest.is_none()
            }
            QualityEvidenceClassV1::Distributional => {
                self.candidate_outcome_digest.is_none()
                    && self.class_certificate_digest.is_none()
                    && self.confidence_scope_digest == Some(self.scope_digest)
            }
            QualityEvidenceClassV1::Unidentified => {
                self.candidate_outcome_digest.is_none()
                    && self.class_certificate_digest.is_none()
                    && self.confidence_scope_digest.is_none()
            }
        };
        if !coherent || !improvement_valid || !detail_valid {
            return Err(QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::InvalidAdmission,
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

    fn expected_digest(&self) -> Result<DigestV1, QualityEnvelopeErrorV1> {
        digest_body(
            ADMISSION_DOMAIN_V1,
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

    pub fn record(&self) -> QualityAdmissionRecordV1 {
        QualityAdmissionRecordV1 {
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

    pub const fn evidence_class(&self) -> QualityEvidenceClassV1 {
        self.evidence_class
    }
    pub const fn selection(&self) -> QualitySelectionV1 {
        self.selection
    }
    pub const fn guarantee(&self) -> QualityGuaranteeV1 {
        self.guarantee
    }
    pub const fn scope_digest(&self) -> DigestV1 {
        self.scope_digest
    }
    pub const fn comparison_identity_digest(&self) -> DigestV1 {
        self.comparison_identity_digest
    }
    pub const fn raw_baseline_identity_digest(&self) -> DigestV1 {
        self.raw_baseline_identity_digest
    }
    pub const fn baseline_outcome_digest(&self) -> DigestV1 {
        self.baseline_outcome_digest
    }
    pub const fn baseline_receipt_digest(&self) -> DigestV1 {
        self.baseline_receipt_digest
    }
    pub const fn candidate_identity_digest(&self) -> Option<DigestV1> {
        self.candidate_identity_digest
    }
    pub const fn candidate_outcome_digest(&self) -> Option<DigestV1> {
        self.candidate_outcome_digest
    }
    pub const fn pairing_method_digest(&self) -> DigestV1 {
        self.pairing_method_digest
    }
    pub const fn protected_predicate_digest(&self) -> DigestV1 {
        self.protected_predicate_digest
    }
    pub const fn verifier_identity_digest(&self) -> DigestV1 {
        self.verifier_identity_digest
    }
    pub const fn class_certificate_digest(&self) -> Option<DigestV1> {
        self.class_certificate_digest
    }
    pub const fn confidence_scope_digest(&self) -> Option<DigestV1> {
        self.confidence_scope_digest
    }
    pub const fn strict_improvement(&self) -> bool {
        self.strict_improvement
    }
    pub const fn evidence_digest(&self) -> DigestV1 {
        self.evidence_digest
    }
    pub const fn digest(&self) -> DigestV1 {
        self.admission_digest
    }
}

pub fn quality_envelope_contract_manifest_v1() -> Value {
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
        "contract_version": QUALITY_ENVELOPE_CONTRACT_VERSION_V1,
        "distributional_arithmetic": "signed_integer_ppm",
        "distributional_strict_selection": "frozen_baseline",
        "linked_capabilities": ["zero_cert::VerifiedEvidence"],
        "max_canonical_bytes": QUALITY_ENVELOPE_MAX_CANONICAL_BYTES_V1,
        "max_dimensions": QUALITY_ENVELOPE_MAX_DIMENSIONS_V1,
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

pub fn quality_envelope_contract_digest_v1() -> DigestV1 {
    domain_digest(
        CONTRACT_DOMAIN_V1,
        canonical_json(&quality_envelope_contract_manifest_v1()).as_bytes(),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QualityEnvelopeFailureCodeV1 {
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
pub struct QualityEnvelopeErrorV1 {
    code: QualityEnvelopeFailureCodeV1,
    detail: String,
}

impl QualityEnvelopeErrorV1 {
    fn new(code: QualityEnvelopeFailureCodeV1, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> QualityEnvelopeFailureCodeV1 {
        self.code
    }
}

impl fmt::Display for QualityEnvelopeErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.detail)
    }
}

impl Error for QualityEnvelopeErrorV1 {}

fn validate_id(field: &str, value: &str) -> Result<(), QualityEnvelopeErrorV1> {
    if value.is_empty()
        || value.len() > QUALITY_ENVELOPE_MAX_METRIC_ID_BYTES_V1
        || value.chars().any(char::is_control)
    {
        return Err(QualityEnvelopeErrorV1::new(
            QualityEnvelopeFailureCodeV1::InvalidProtectedVector,
            format!("{field} is empty, contains control characters, or exceeds its bound"),
        ));
    }
    Ok(())
}

fn require_nonzero(label: &str, digests: &[DigestV1]) -> Result<(), QualityEnvelopeErrorV1> {
    if digests.contains(&DigestV1::ZERO) {
        Err(QualityEnvelopeErrorV1::new(
            QualityEnvelopeFailureCodeV1::MissingBinding,
            format!("{label} contains a zero digest"),
        ))
    } else {
        Ok(())
    }
}

fn missing_binding(label: &str) -> QualityEnvelopeErrorV1 {
    QualityEnvelopeErrorV1::new(
        QualityEnvelopeFailureCodeV1::MissingBinding,
        format!("{label} digest is zero"),
    )
}

fn version_error(label: &str) -> QualityEnvelopeErrorV1 {
    QualityEnvelopeErrorV1::new(
        QualityEnvelopeFailureCodeV1::SchemaVersionMismatch,
        format!("{label} contract version is not current"),
    )
}

fn require_version_and_digest(
    version: u16,
    actual: DigestV1,
    expected: DigestV1,
    label: &str,
) -> Result<(), QualityEnvelopeErrorV1> {
    if version != QUALITY_ENVELOPE_CONTRACT_VERSION_V1 {
        return Err(version_error(label));
    }
    if actual == DigestV1::ZERO || actual != expected {
        return Err(QualityEnvelopeErrorV1::new(
            QualityEnvelopeFailureCodeV1::CertificateDigestMismatch,
            format!("{label} digest does not match its canonical body"),
        ));
    }
    Ok(())
}

fn require_exact_payload(
    label: &str,
    expected: &[u8],
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<(), QualityEnvelopeErrorV1> {
    if evidence.certificate().payload.as_ref() != expected {
        return Err(QualityEnvelopeErrorV1::new(
            QualityEnvelopeFailureCodeV1::EvidencePayloadMismatch,
            format!("verified evidence payload does not equal canonical {label} bytes"),
        ));
    }
    Ok(())
}

fn evidence_digest(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<DigestV1, QualityEnvelopeErrorV1> {
    evidence
        .certificate()
        .canonical_digest()
        .map(DigestV1::from_bytes)
        .map_err(|error| {
            QualityEnvelopeErrorV1::new(
                QualityEnvelopeFailureCodeV1::EvidenceInvalid,
                error.to_string(),
            )
        })
}

fn verifier_identity_digest(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<DigestV1, QualityEnvelopeErrorV1> {
    let provenance = &evidence.certificate().provenance;
    digest_body(
        VERIFIER_DOMAIN_V1,
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

fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, QualityEnvelopeErrorV1> {
    let value = serde_json::to_value(value).map_err(json_error)?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() > QUALITY_ENVELOPE_MAX_CANONICAL_BYTES_V1 {
        return Err(QualityEnvelopeErrorV1::new(
            QualityEnvelopeFailureCodeV1::CanonicalPayloadTooLarge,
            "canonical quality payload exceeds its byte bound",
        ));
    }
    Ok(bytes)
}

fn digest_body(domain: &[u8], value: Value) -> Result<DigestV1, QualityEnvelopeErrorV1> {
    Ok(domain_digest(domain, canonical_json(&value).as_bytes()))
}

fn domain_digest(domain: &[u8], payload: &[u8]) -> DigestV1 {
    let mut bytes = Vec::with_capacity(domain.len() + payload.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(payload);
    DigestV1::from_bytes(sha256(&bytes))
}

fn json_error(error: serde_json::Error) -> QualityEnvelopeErrorV1 {
    QualityEnvelopeErrorV1::new(
        QualityEnvelopeFailureCodeV1::SerializationFailure,
        error.to_string(),
    )
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-gate/unit/quality.rs"]
mod tests;
