//! Inline decision gate for the plan interpreter (ZS-EXEC-003/004/007).
//!
//! Plans declare semantic decision points explicitly by calling
//! `zero.decision.require(point, observed_value)`. The gate resolves the
//! observation against the attached contingent policy and returns the
//! selected alternative WITHOUT executing either branch privately.
//!
//! Fail-closed law (V6-C03/H03): with no policy attached, or with no rule
//! covering the observation, resolution is `DecisionRequired` -- the
//! interpreter aborts the plan with a typed [`zero_abi::DecisionRequired`]
//! payload instead of silently choosing a branch. A rule selecting an
//! alternative the decision point does not offer is a policy error and
//! aborts loudly as well; it is never a silent selection.

use std::cell::{Cell, RefCell};

use zero_abi::{
    ContingentPolicy, DecisionError, DecisionRequired, ObservationClass, PolicyResolution,
    SemanticDecisionPoint,
};

pub const DECISION_SURFACE: &str = "decision";
pub const DECISION_REQUIRE_METHOD: &str = "require";

/// One rule of the attached contingent policy and how often it matched
/// during one execution (V6-R3, ZS-EXEC-004/007). Matched means the rule's
/// observation class AND observed-match condition both accepted an
/// observation; a rule that matched but selected an unoffered alternative
/// still counts, because the loud policy error is reported separately at
/// the abort.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct GateRuleUsage {
    /// Index of the rule in the attached policy's `rules` vector.
    pub rule_index: usize,
    /// The observation class the rule targets.
    pub observation_class: ObservationClass,
    /// How many `decision.require` observations this rule matched.
    pub matched_observations: u64,
}

/// Honest usage report of the contingent policy over one execution
/// (V6-R3, ZS-EXEC-004/007): every rule is reported with its match count
/// and the rules that never matched are listed explicitly, so a caller can
/// prove which policy rules were exercised -- nothing is silently dropped.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct GateUsageReport {
    /// One entry per policy rule, in policy order.
    pub rules: Vec<GateRuleUsage>,
    /// Indexes into the policy `rules` vector of rules that never matched a
    /// single observation during the execution. Empty when every rule was
    /// exercised at least once.
    pub unused_rule_indexes: Vec<usize>,
    /// Total `decision.require` observations the gate resolved during the
    /// execution (matched and uncovered alike).
    pub observations: u64,
}

/// The policy and authority attached to one host execution.
///
/// `DecisionGate::default()` carries no policy, which is the fail-closed
/// state: every `decision.require` observation resolves to `DecisionRequired`
/// and execution stops.
#[derive(Debug)]
pub struct DecisionGate {
    policy: Option<ContingentPolicy>,
    /// Per-rule matched-observation counters aligned with `policy.rules`.
    /// The interpreter is single-threaded and the gate is only consulted
    /// while a plan runs, so plain interior cells are safe.
    usage: RefCell<Vec<u64>>,
    /// Total observations resolved (matched and uncovered alike).
    resolved: Cell<u64>,
}

impl Default for DecisionGate {
    fn default() -> Self {
        Self {
            policy: None,
            usage: RefCell::new(Vec::new()),
            resolved: Cell::new(0),
        }
    }
}

impl Clone for DecisionGate {
    /// A cloned gate is a fresh gate with the same policy: usage counters
    /// are reset so a clone never inherits another execution's report.
    fn clone(&self) -> Self {
        Self {
            policy: self.policy.clone(),
            usage: RefCell::new(vec![0; self.policy.as_ref().map_or(0, |policy| policy.rules.len())]),
            resolved: Cell::new(0),
        }
    }
}

impl DecisionGate {
    pub fn new(policy: Option<ContingentPolicy>) -> Self {
        Self {
            usage: RefCell::new(vec![0; policy.as_ref().map_or(0, |policy| policy.rules.len())]),
            resolved: Cell::new(0),
            policy,
        }
    }

    pub fn policy(&self) -> Option<&ContingentPolicy> {
        self.policy.as_ref()
    }

    /// Fail-closed resolution of one semantic decision point.
    ///
    /// - No policy attached -> `DecisionRequired` (never select privately).
    /// - Policy covers the observation with an offered alternative ->
    ///   `Selected`, and the matching rule's usage counter advances.
    /// - Policy covers the observation but selects an unoffered alternative
    ///   -> `PolicyError` (fail closed, never a silent selection); the
    ///   matching rule still counts as matched, the error is surfaced
    ///   separately.
    /// - Policy does not cover the observation -> `DecisionRequired`.
    pub fn resolve(
        &self,
        point: &SemanticDecisionPoint,
        observed_value: &str,
    ) -> GateResolution {
        self.resolved.set(self.resolved.get().saturating_add(1));
        let Some(policy) = &self.policy else {
            return GateResolution::DecisionRequired(decision_required_for(point, observed_value));
        };
        match policy.resolve(point, observed_value) {
            PolicyResolution::Selected {
                alternative,
                rule_index,
            } => {
                self.record_match(rule_index);
                GateResolution::Selected(alternative)
            }
            PolicyResolution::Uncovered { decision_required } => {
                GateResolution::DecisionRequired(decision_required)
            }
            PolicyResolution::PolicyError(error) => {
                if let DecisionError::AlternativeNotOffered { rule_index, .. } = &error {
                    self.record_match(*rule_index);
                }
                GateResolution::PolicyError(error)
            }
        }
    }

    /// Record one matched observation for the rule at `rule_index`. The
    /// gate is single-threaded (the interpreter holds the only reference
    /// while a plan runs), so an overlapping borrow is a programming error
    /// and panics loudly instead of dropping the count.
    fn record_match(&self, rule_index: usize) {
        let mut usage = self.usage.borrow_mut();
        if let Some(counter) = usage.get_mut(rule_index) {
            *counter = counter.saturating_add(1);
        }
    }

    /// Honest per-rule usage report of the attached policy (V6-R3): every
    /// rule with its match count, plus the explicit list of rules that
    /// never matched. `None` when no policy is attached -- there is nothing
    /// to report and no coverage claim to check.
    pub fn usage_report(&self) -> Option<GateUsageReport> {
        let policy = self.policy.as_ref()?;
        let usage = self.usage.borrow();
        let rules = policy
            .rules
            .iter()
            .enumerate()
            .map(|(rule_index, rule)| GateRuleUsage {
                rule_index,
                observation_class: rule.observation_class.clone(),
                matched_observations: usage.get(rule_index).copied().unwrap_or(0),
            })
            .collect::<Vec<_>>();
        let unused_rule_indexes = rules
            .iter()
            .filter(|entry| entry.matched_observations == 0)
            .map(|entry| entry.rule_index)
            .collect();
        Some(GateUsageReport {
            observations: self.resolved.get(),
            rules,
            unused_rule_indexes,
        })
    }
}

/// Build the `DecisionRequired` payload for an uncovered observation.
fn decision_required_for(
    point: &SemanticDecisionPoint,
    observed_value: &str,
) -> DecisionRequired {
    DecisionRequired {
        decision_id: point.decision_id.clone(),
        observation_class: point.observation_class.clone(),
        question: point.question.clone(),
        choices: point.alternatives.clone(),
        observed_value: observed_value.to_owned(),
    }
}

/// Resolution of one `decision.require` call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GateResolution {
    /// A covering rule selected an offered alternative; execution continues.
    Selected(String),
    /// No rule covered the observation (or no policy exists); execution must
    /// stop and surface the payload as `DecisionRequired`.
    DecisionRequired(DecisionRequired),
    /// A rule selected an unoffered alternative; the policy is defective and
    /// execution must stop loudly.
    PolicyError(DecisionError),
}

