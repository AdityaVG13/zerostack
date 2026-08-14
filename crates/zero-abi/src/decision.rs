//! Typed contingent policy over observation classes (ZS-EXEC-004).
//!
//! A plan reaching a semantic decision point calls the decision surface with
//! the point and the observed value. If the supplied contingent policy has a
//! covering rule, the selected alternative is returned and execution stays
//! within one call. Otherwise the resolver returns an `Uncovered`
//! `DecisionRequiredV1` payload -- the interpreter must NOT privately choose a
//! branch (V6-C03/H03). A rule that selects an alternative the decision point
//! does not offer is a policy error and fails closed ([`DecisionErrorV1::AlternativeNotOffered`]),
//! never a silent selection.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use super::verdict::SafetyVerdictV1;

pub const OBSERVATION_CLASS_MAX_BYTES: usize = 128;
pub const DECISION_ID_MAX_BYTES: usize = 256;

/// Fail-closed error for decision-point and policy construction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecisionErrorV1 {
    InvalidObservationClass(String),
    InvalidDecisionPoint(String),
    InvalidPolicyRule(String),
    AlternativeNotOffered { decision_id: String, alternative: String },
    InvalidPolicy(String),
}

impl fmt::Display for DecisionErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidObservationClass(detail) => {
                write!(formatter, "invalid observation class: {detail}")
            }
            Self::InvalidDecisionPoint(detail) => {
                write!(formatter, "invalid decision point: {detail}")
            }
            Self::InvalidPolicyRule(detail) => write!(formatter, "invalid policy rule: {detail}"),
            Self::AlternativeNotOffered {
                decision_id,
                alternative,
            } => write!(
                formatter,
                "policy rule selects alternative {alternative} not offered by decision point {decision_id}"
            ),
            Self::InvalidPolicy(detail) => write!(formatter, "invalid policy: {detail}"),
        }
    }
}

impl Error for DecisionErrorV1 {}

/// An observation class is the stable identity of one kind of semantic
/// observation (for example `"branch.test_suite"` or `"api.breaking_change"`).
/// Grammar: nonempty, at most 128 bytes, lowercase `[a-z0-9_.-]` only.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationClassV1 {
    pub class_id: String,
}

impl ObservationClassV1 {
    pub fn new(class_id: impl Into<String>) -> Result<Self, DecisionErrorV1> {
        let class = Self {
            class_id: class_id.into(),
        };
        class.validate()?;
        Ok(class)
    }

    pub fn validate(&self) -> Result<(), DecisionErrorV1> {
        if self.class_id.is_empty() {
            return Err(DecisionErrorV1::InvalidObservationClass(
                "class_id must be nonempty".into(),
            ));
        }
        if self.class_id.len() > OBSERVATION_CLASS_MAX_BYTES {
            return Err(DecisionErrorV1::InvalidObservationClass(format!(
                "class_id is {} bytes, maximum {OBSERVATION_CLASS_MAX_BYTES}",
                self.class_id.len()
            )));
        }
        if !self
            .class_id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err(DecisionErrorV1::InvalidObservationClass(
                "class_id must match lowercase [a-z0-9_.-]".into(),
            ));
        }
        Ok(())
    }
}

/// One semantic decision point: the question asked of the model when the
/// observation cannot be resolved mechanically.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticDecisionPointV1 {
    pub decision_id: String,
    pub observation_class: ObservationClassV1,
    pub question: String,
    pub alternatives: Vec<String>,
    pub evidence_refs: Vec<String>,
}

impl SemanticDecisionPointV1 {
    pub fn new(
        decision_id: impl Into<String>,
        observation_class: ObservationClassV1,
        question: impl Into<String>,
        alternatives: Vec<String>,
        evidence_refs: Vec<String>,
    ) -> Result<Self, DecisionErrorV1> {
        let point = Self {
            decision_id: decision_id.into(),
            observation_class,
            question: question.into(),
            alternatives,
            evidence_refs,
        };
        point.validate()?;
        Ok(point)
    }

    /// Fail-closed validation: nonempty bounded decision id and question, at
    /// least one alternative, no duplicate alternatives, and nonempty
    /// evidence refs when present.
    pub fn validate(&self) -> Result<(), DecisionErrorV1> {
        if self.decision_id.is_empty() {
            return Err(DecisionErrorV1::InvalidDecisionPoint(
                "decision_id must be nonempty".into(),
            ));
        }
        if self.decision_id.len() > DECISION_ID_MAX_BYTES {
            return Err(DecisionErrorV1::InvalidDecisionPoint(format!(
                "decision_id is {} bytes, maximum {DECISION_ID_MAX_BYTES}",
                self.decision_id.len()
            )));
        }
        if self.question.is_empty() {
            return Err(DecisionErrorV1::InvalidDecisionPoint(
                "question must be nonempty".into(),
            ));
        }
        if self.alternatives.is_empty() {
            return Err(DecisionErrorV1::InvalidDecisionPoint(
                "alternatives must be nonempty".into(),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for alternative in &self.alternatives {
            if alternative.is_empty() {
                return Err(DecisionErrorV1::InvalidDecisionPoint(
                    "alternatives must not be empty strings".into(),
                ));
            }
            if !seen.insert(alternative.as_str()) {
                return Err(DecisionErrorV1::InvalidDecisionPoint(format!(
                    "duplicate alternative {alternative:?}"
                )));
            }
        }
        if self.evidence_refs.iter().any(|reference| reference.is_empty()) {
            return Err(DecisionErrorV1::InvalidDecisionPoint(
                "evidence_refs must not be empty strings".into(),
            ));
        }
        self.observation_class.validate()?;
        Ok(())
    }
}

/// How a policy rule matches the observed value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservedMatchV1 {
    Exact { value: String },
    Any,
}

/// One contingent policy rule: for a given observation class, when the
/// observed value matches, select the named alternative.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContingentPolicyRuleV1 {
    pub observation_class: ObservationClassV1,
    pub observed: ObservedMatchV1,
    pub select_alternative: String,
}

impl ContingentPolicyRuleV1 {
    pub fn new(
        observation_class: ObservationClassV1,
        observed: ObservedMatchV1,
        select_alternative: impl Into<String>,
    ) -> Result<Self, DecisionErrorV1> {
        let rule = Self {
            observation_class,
            observed,
            select_alternative: select_alternative.into(),
        };
        rule.validate()?;
        Ok(rule)
    }

    pub fn validate(&self) -> Result<(), DecisionErrorV1> {
        self.observation_class.validate()?;
        if let ObservedMatchV1::Exact { value } = &self.observed
            && value.is_empty()
        {
            return Err(DecisionErrorV1::InvalidPolicyRule(
                "exact observed value must be nonempty".into(),
            ));
        }
        if self.select_alternative.is_empty() {
            return Err(DecisionErrorV1::InvalidPolicyRule(
                "select_alternative must be nonempty".into(),
            ));
        }
        Ok(())
    }
}

/// The payload a `DecisionRequired` envelope carries: the question, the
/// offered choices, the observation class, and the observed value that no
/// rule covered.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequiredV1 {
    pub decision_id: String,
    pub observation_class: ObservationClassV1,
    pub question: String,
    pub choices: Vec<String>,
    pub observed_value: String,
}

/// A contingent policy is a total plan of rules over observation classes.
/// Empty rule sets are legal but resolve everything to `Uncovered` -- a
/// caller claiming totality must prove coverage separately.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContingentPolicyV1 {
    pub rules: Vec<ContingentPolicyRuleV1>,
}

impl ContingentPolicyV1 {
    pub fn new(rules: Vec<ContingentPolicyRuleV1>) -> Result<Self, DecisionErrorV1> {
        let policy = Self { rules };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<(), DecisionErrorV1> {
        for rule in &self.rules {
            rule.validate()
                .map_err(|error| DecisionErrorV1::InvalidPolicyRule(error.to_string()))?;
        }
        Ok(())
    }

    /// Resolve one semantic decision point against this policy.
    ///
    /// Laws:
    /// 1. The first rule whose observation class matches the point AND whose
    ///    observed match accepts the observed value selects its alternative
    ///    -- but ONLY if that alternative is offered by the point.
    /// 2. A matching rule whose alternative is not offered by the point is a
    ///    policy error ([`DecisionErrorV1::AlternativeNotOffered`]) and fails
    ///    closed; it is never a silent selection.
    /// 3. No matching rule resolves to `Uncovered` with the full
    ///    `DecisionRequiredV1` payload.
    pub fn resolve(
        &self,
        point: &SemanticDecisionPointV1,
        observed_value: &str,
    ) -> PolicyResolutionV1 {
        for rule in &self.rules {
            if rule.observation_class != point.observation_class {
                continue;
            }
            let matched = match &rule.observed {
                ObservedMatchV1::Exact { value } => value == observed_value,
                ObservedMatchV1::Any => true,
            };
            if !matched {
                continue;
            }
            if !point
                .alternatives
                .iter()
                .any(|alternative| alternative == &rule.select_alternative)
            {
                return PolicyResolutionV1::PolicyError(DecisionErrorV1::AlternativeNotOffered {
                    decision_id: point.decision_id.clone(),
                    alternative: rule.select_alternative.clone(),
                });
            }
            return PolicyResolutionV1::Selected {
                alternative: rule.select_alternative.clone(),
            };
        }
        PolicyResolutionV1::Uncovered {
            decision_required: DecisionRequiredV1 {
                decision_id: point.decision_id.clone(),
                observation_class: point.observation_class.clone(),
                question: point.question.clone(),
                choices: point.alternatives.clone(),
                observed_value: observed_value.to_owned(),
            },
        }
    }

    /// Whether this policy is a pure safety fallback: no rule exists at all.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// Result of resolving a decision point against a contingent policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolicyResolutionV1 {
    /// A covering rule selected an offered alternative. This is the ONLY
    /// resolution that lets execution continue within one call.
    Selected { alternative: String },
    /// No rule covered the observation; the decision must be surfaced as
    /// `DecisionRequired` and execution must stop (no private selection).
    Uncovered { decision_required: DecisionRequiredV1 },
    /// A rule matched but selected an unoffered alternative. Fail closed:
    /// the policy itself is defective and nothing may be selected.
    PolicyError(DecisionErrorV1),
}

/// Bridge from the shared trivalent verdict into the contingent-policy
/// vocabulary: a decision point whose safety verdict is `Safe` may be
/// resolved mechanically; `Unsafe` and `Unknown` never authorize a selection.
pub fn verdict_permits_selection(verdict: &SafetyVerdictV1) -> bool {
    matches!(verdict, SafetyVerdictV1::Safe)
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-abi/unit/decision.rs"]
mod tests;
