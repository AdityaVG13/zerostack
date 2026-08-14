//! Unified verification verdict for protected scopes.
//!
//! Every protected-scope comparison resolves to one of four verdict kinds:
//! `Equivalent`, `Dominates`, `Reject`, or `Unknown`. Only `Equivalent` and
//! `Dominates` grant candidate authority. `Unknown` is terminal-epistemic:
//! it always routes to the frozen raw-baseline fallback, and NOTHING in this
//! crate promotes `Unknown` to `Equivalent` or `Dominates`. Disagreement,
//! verifier timeout, uncovered protected dimensions, and distributional
//! evidence all resolve to `Unknown`; the fallback decision is never laundered
//! into a passing verdict.
//!
//! Subjective dimensions (ergonomics, taste, and other non-mechanical
//! comparisons) cannot be admitted by evidence alone. A dimension without a
//! declared evaluator forces a `DecisionRequired` admission before the
//! underlying verdict is even consulted -- a dominating verdict cannot pass a
//! subjective dimension that no declared evaluator attested.

use std::{collections::BTreeSet, error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::quality::QualityEvidenceClassV1;

pub const VERDICT_CONTRACT_VERSION_V1: u16 = 1;
pub const VERDICT_MAX_EVALUATOR_ID_BYTES_V1: usize = 128;
pub const VERDICT_DIGEST_HEX_LEN_V1: usize = 64;

const REASON_VERIFIER_DISAGREEMENT: &str = "verifier_disagreement";
const REASON_NO_DECLARED_EVALUATOR: &str = "subjective_dimension_requires_declared_evaluator";
const REASON_DUPLICATE_DIMENSION: &str = "duplicate_subjective_dimension";
const REASON_INVALID_DIMENSION: &str = "invalid_subjective_dimension";
const REASON_INVALID_EVALUATOR: &str = "invalid_declared_evaluator";

/// Failure codes for verdict construction and identity validation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictFailureCodeV1 {
    EmptyReasons,
    InvalidEvaluatorIdentity,
    InvalidSubjectiveDimension,
}

/// Fail-closed error for verdict and subjective-gate construction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerdictErrorV1 {
    code: VerdictFailureCodeV1,
    detail: String,
}

impl VerdictErrorV1 {
    fn new(code: VerdictFailureCodeV1, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> VerdictFailureCodeV1 {
        self.code
    }
}

impl fmt::Display for VerdictErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.detail)
    }
}

impl Error for VerdictErrorV1 {}

/// The unified verification verdict for one protected scope.
///
/// Wire shape:
/// - `"equivalent"` and `"dominates"` are the only authority-granting kinds;
/// - `"reject"` carries the rejection reasons;
/// - `"unknown"` carries the epistemic reasons and is terminal: nothing in
///   this crate ever promotes it, and callers must route it to the frozen
///   raw-baseline fallback.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum VerifierVerdictV1 {
    Equivalent,
    Dominates,
    Reject {
        reasons: Vec<String>,
    },
    Unknown {
        reasons: Vec<String>,
    },
}

impl VerifierVerdictV1 {
    /// Construct a `Reject` verdict. Reasons are sorted, deduplicated, and
    /// must be nonempty; empty or blank reason sets fail closed.
    pub fn reject(reasons: Vec<String>) -> Result<Self, VerdictErrorV1> {
        let reasons = normalize_reasons(reasons)?;
        Ok(Self::Reject { reasons })
    }

    /// Construct an `Unknown` verdict. Reasons are sorted, deduplicated, and
    /// must be nonempty; empty or blank reason sets fail closed.
    pub fn unknown(reasons: Vec<String>) -> Result<Self, VerdictErrorV1> {
        let reasons = normalize_reasons(reasons)?;
        Ok(Self::Unknown { reasons })
    }

    /// Map raw quality-evidence classes onto the unified verdict.
    ///
    /// Exact-neutral evidence is `Equivalent`; pointwise and scoped-class
    /// dominance evidence are `Dominates`. Distributional and unidentified
    /// evidence are `Unknown` with a fixed reason: a population claim is never
    /// laundered into an individual proof, and an unidentified class is never
    /// silently treated as authority.
    pub fn from_quality_evidence(class: QualityEvidenceClassV1) -> Self {
        match class {
            QualityEvidenceClassV1::ExactNeutral => Self::Equivalent,
            QualityEvidenceClassV1::PointwiseDominance | QualityEvidenceClassV1::ScopedClassDominance => {
                Self::Dominates
            }
            QualityEvidenceClassV1::Distributional => Self::Unknown {
                reasons: vec!["distributional_evidence_is_not_pointwise_proof".into()],
            },
            QualityEvidenceClassV1::Unidentified => Self::Unknown {
                reasons: vec!["unidentified_quality_evidence".into()],
            },
        }
    }

    /// A verifier that timed out yields `Unknown` for the protected scope.
    pub fn from_verifier_timeout(verifier_id: &str) -> Self {
        Self::Unknown {
            reasons: vec![format!("verifier_timeout:{verifier_id}")],
        }
    }

    /// Merge two verifier verdicts for the same protected scope.
    ///
    /// Agreeing verdicts are preserved. Two `Unknown` verdicts merge their
    /// reasons. Two `Reject` verdicts merge their reasons and stay rejected.
    /// Any other disagreement -- including `Equivalent` vs `Dominates` and
    /// `Reject` vs any passing side -- is `Unknown`: the passing side is never
    /// silently kept when another verifier disagrees.
    pub fn from_verifier_disagreement(
        a: &VerifierVerdictV1,
        b: &VerifierVerdictV1,
    ) -> VerifierVerdictV1 {
        match (a, b) {
            (Self::Unknown { .. }, Self::Unknown { .. }) => {
                let mut reasons = a.reasons().to_vec();
                reasons.extend_from_slice(b.reasons());
                Self::Unknown {
                    reasons: merge_reasons(reasons),
                }
            }
            (Self::Reject { .. }, Self::Reject { .. }) => {
                let mut reasons = a.reasons().to_vec();
                reasons.extend_from_slice(b.reasons());
                Self::Reject {
                    reasons: merge_reasons(reasons),
                }
            }
            (a, b) if a == b => a.clone(),
            (a, b) => Self::Unknown {
                reasons: vec![
                    REASON_VERIFIER_DISAGREEMENT.into(),
                    format!("left_kind={}", a.kind_label()),
                    format!("right_kind={}", b.kind_label()),
                ],
            },
        }
    }

    /// A protected dimension no verifier covered is `Unknown` for that scope.
    pub fn from_uncovered_dimension(dimension: &str) -> Self {
        Self::Unknown {
            reasons: vec![format!("uncovered_protected_dimension:{dimension}")],
        }
    }

    /// Whether this verdict authorizes the candidate over the protected scope.
    /// Only `Equivalent` and `Dominates` grant authority; `Reject` and
    /// `Unknown` never do.
    pub fn grants_candidate_authority(&self) -> bool {
        matches!(self, Self::Equivalent | Self::Dominates)
    }

    /// Short stable label for the verdict kind, used in disagreement reasons
    /// and rendering: `equivalent`, `dominates`, `reject`, or `unknown`.
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Equivalent => "equivalent",
            Self::Dominates => "dominates",
            Self::Reject { .. } => "reject",
            Self::Unknown { .. } => "unknown",
        }
    }

    /// The reasons carried by this verdict (empty for the unit kinds).
    pub fn reasons(&self) -> &[String] {
        match self {
            Self::Equivalent | Self::Dominates => &[],
            Self::Reject { reasons } | Self::Unknown { reasons } => reasons,
        }
    }
}

/// Identity of the human or verifier that declared a subjective dimension.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluatorIdentityV1 {
    pub evaluator_id: String,
    pub declaration_digest_hex: String,
}

impl EvaluatorIdentityV1 {
    /// Validate the identity: a nonempty id of at most 128 bytes and a
    /// declaration digest that is exactly 64 lowercase hex characters.
    pub fn validate(&self) -> Result<(), VerdictErrorV1> {
        if self.evaluator_id.is_empty()
            || self.evaluator_id.len() > VERDICT_MAX_EVALUATOR_ID_BYTES_V1
            || self.evaluator_id.chars().any(char::is_control)
        {
            return Err(VerdictErrorV1::new(
                VerdictFailureCodeV1::InvalidEvaluatorIdentity,
                "evaluator_id must be nonempty, at most 128 bytes, and free of control characters",
            ));
        }
        if self.declaration_digest_hex.len() != VERDICT_DIGEST_HEX_LEN_V1
            || !self
                .declaration_digest_hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(VerdictErrorV1::new(
                VerdictFailureCodeV1::InvalidEvaluatorIdentity,
                "declaration_digest_hex must be exactly 64 lowercase hex characters",
            ));
        }
        Ok(())
    }
}

/// One subjective protected dimension, optionally bound to its declared
/// evaluator. A dimension without a declared evaluator can never be admitted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectiveDimensionV1 {
    pub name: String,
    pub declared_evaluator: Option<EvaluatorIdentityV1>,
}

impl SubjectiveDimensionV1 {
    /// Validate only the dimension name: nonempty and free of control
    /// characters. Evaluator validation is intentionally separate so the
    /// subjective gate can distinguish `invalid_subjective_dimension` from
    /// `invalid_declared_evaluator` (an invalid evaluator is not a malformed
    /// dimension).
    pub fn validate_name(&self) -> Result<(), VerdictErrorV1> {
        if self.name.is_empty() || self.name.chars().any(char::is_control) {
            return Err(VerdictErrorV1::new(
                VerdictFailureCodeV1::InvalidSubjectiveDimension,
                "dimension name must be nonempty and free of control characters",
            ));
        }
        Ok(())
    }

    /// Validate the dimension: name and, when present, declared evaluator.
    pub fn validate(&self) -> Result<(), VerdictErrorV1> {
        self.validate_name()?;
        if let Some(evaluator) = &self.declared_evaluator {
            evaluator.validate()?;
        }
        Ok(())
    }
}

/// Outcome of applying the subjective gate to a verdict.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum GateAdmissionV1 {
    /// The subjective gate is satisfied; the verdict itself still gates
    /// authority via [`VerifierVerdictV1::grants_candidate_authority`].
    Admitted {
        verdict: VerifierVerdictV1,
    },
    /// A decision is required before this protected scope can be admitted.
    DecisionRequired {
        dimension: String,
        reason: String,
    },
}

/// Apply the subjective gate to a verification verdict.
///
/// Fail-closed law:
/// 1. Duplicate dimension names yield `DecisionRequired`
///    (`duplicate_subjective_dimension`) before anything else.
/// 2. Any dimension with `declared_evaluator: None` yields `DecisionRequired`
///    (`subjective_dimension_requires_declared_evaluator`) -- even a
///    dominating verdict cannot pass an unattested subjective dimension.
/// 3. Malformed dimensions or invalid declared evaluators fail closed with
///    `invalid_subjective_dimension` / `invalid_declared_evaluator`.
/// 4. With every dimension declared and valid, the admission preserves the
///    verdict; authority still requires
///    [`VerifierVerdictV1::grants_candidate_authority`].
pub fn admit_with_subjective_gate(
    verdict: VerifierVerdictV1,
    subjective_dimensions: &[SubjectiveDimensionV1],
) -> GateAdmissionV1 {
    let mut seen = BTreeSet::new();
    for dimension in subjective_dimensions {
        if !seen.insert(dimension.name.as_str()) {
            return GateAdmissionV1::DecisionRequired {
                dimension: dimension.name.clone(),
                reason: REASON_DUPLICATE_DIMENSION.into(),
            };
        }
    }
    for dimension in subjective_dimensions {
        if dimension.validate_name().is_err() {
            return GateAdmissionV1::DecisionRequired {
                dimension: dimension.name.clone(),
                reason: REASON_INVALID_DIMENSION.into(),
            };
        }
        let Some(evaluator) = &dimension.declared_evaluator else {
            return GateAdmissionV1::DecisionRequired {
                dimension: dimension.name.clone(),
                reason: REASON_NO_DECLARED_EVALUATOR.into(),
            };
        };
        if evaluator.validate().is_err() {
            return GateAdmissionV1::DecisionRequired {
                dimension: dimension.name.clone(),
                reason: REASON_INVALID_EVALUATOR.into(),
            };
        }
    }
    GateAdmissionV1::Admitted { verdict }
}

/// Sort and deduplicate reasons; reject empty or blank reason sets.
fn normalize_reasons(reasons: Vec<String>) -> Result<Vec<String>, VerdictErrorV1> {
    if reasons.is_empty() {
        return Err(VerdictErrorV1::new(
            VerdictFailureCodeV1::EmptyReasons,
            "reasons must be nonempty",
        ));
    }
    if reasons.iter().any(|reason| reason.trim().is_empty()) {
        return Err(VerdictErrorV1::new(
            VerdictFailureCodeV1::EmptyReasons,
            "reasons must not be blank",
        ));
    }
    Ok(merge_reasons(reasons))
}

/// Sort and deduplicate a reason list. Inputs are pre-validated nonempty, so
/// this internal merge cannot produce an empty set.
fn merge_reasons(mut reasons: Vec<String>) -> Vec<String> {
    debug_assert!(!reasons.is_empty());
    debug_assert!(reasons.iter().all(|reason| !reason.trim().is_empty()));
    reasons.sort();
    reasons.dedup();
    reasons
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-gate/unit/verdict.rs"]
mod tests;
