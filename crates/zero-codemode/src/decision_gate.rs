//! Inline decision gate for the plan interpreter (ZS-EXEC-003/004/007).
//!
//! Plans declare semantic decision points explicitly by calling
//! `zero.decision.require(point, observed_value)`. The gate resolves the
//! observation against the attached contingent policy and returns the
//! selected alternative WITHOUT executing either branch privately.
//!
//! Fail-closed law (V6-C03/H03): with no policy attached, or with no rule
//! covering the observation, resolution is `DecisionRequired` -- the
//! interpreter aborts the plan with a typed [`zero_abi::DecisionRequiredV1`]
//! payload instead of silently choosing a branch. A rule selecting an
//! alternative the decision point does not offer is a policy error and
//! aborts loudly as well; it is never a silent selection.

use zero_abi::{
    ContingentPolicyV1, DecisionErrorV1, DecisionRequiredV1, PolicyResolutionV1,
    SemanticDecisionPointV1,
};

pub const DECISION_SURFACE: &str = "decision";
pub const DECISION_REQUIRE_METHOD: &str = "require";

/// The policy and authority attached to one host execution.
///
/// `DecisionGate::default()` carries no policy, which is the fail-closed
/// state: every `decision.require` observation resolves to `DecisionRequired`
/// and execution stops.
#[derive(Clone, Debug, Default)]
pub struct DecisionGate {
    policy: Option<ContingentPolicyV1>,
}

impl DecisionGate {
    pub fn new(policy: Option<ContingentPolicyV1>) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> Option<&ContingentPolicyV1> {
        self.policy.as_ref()
    }

    /// Fail-closed resolution of one semantic decision point.
    ///
    /// - No policy attached -> `DecisionRequired` (never select privately).
    /// - Policy covers the observation with an offered alternative ->
    ///   `Selected`.
    /// - Policy covers the observation but selects an unoffered alternative
    ///   -> `PolicyError` (fail closed, never a silent selection).
    /// - Policy does not cover the observation -> `DecisionRequired`.
    pub fn resolve(
        &self,
        point: &SemanticDecisionPointV1,
        observed_value: &str,
    ) -> GateResolutionV1 {
        let Some(policy) = &self.policy else {
            return GateResolutionV1::DecisionRequired(decision_required_for(point, observed_value));
        };
        match policy.resolve(point, observed_value) {
            PolicyResolutionV1::Selected { alternative } => GateResolutionV1::Selected(alternative),
            PolicyResolutionV1::Uncovered { decision_required } => {
                GateResolutionV1::DecisionRequired(decision_required)
            }
            PolicyResolutionV1::PolicyError(error) => GateResolutionV1::PolicyError(error),
        }
    }
}

/// Build the `DecisionRequired` payload for an uncovered observation.
fn decision_required_for(
    point: &SemanticDecisionPointV1,
    observed_value: &str,
) -> DecisionRequiredV1 {
    DecisionRequiredV1 {
        decision_id: point.decision_id.clone(),
        observation_class: point.observation_class.clone(),
        question: point.question.clone(),
        choices: point.alternatives.clone(),
        observed_value: observed_value.to_owned(),
    }
}

/// Resolution of one `decision.require` call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GateResolutionV1 {
    /// A covering rule selected an offered alternative; execution continues.
    Selected(String),
    /// No rule covered the observation (or no policy exists); execution must
    /// stop and surface the payload as `DecisionRequired`.
    DecisionRequired(DecisionRequiredV1),
    /// A rule selected an unoffered alternative; the policy is defective and
    /// execution must stop loudly.
    PolicyError(DecisionErrorV1),
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-codemode/unit/decision_gate.rs"]
mod tests;
