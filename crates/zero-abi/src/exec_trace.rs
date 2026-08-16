//! Execution trace surface with equivalence comparison (V6-R15,
//! ZS-EXEC-002/005).
//!
//! An [`ExecTraceV1`] is the deterministic, mode-independent record of one
//! settled execution of a plan DAG: nodes in deterministic topological
//! order, each with its outcome and the protected decision info the model
//! saw at that node. Same plan digest + same input digest => equivalent
//! trace; any divergence is reported loudly as a typed
//! [`TraceDivergenceV1`] naming the first diverging record, field, and
//! expected/actual values -- never a silent boolean.

use serde::{Deserialize, Serialize};

use crate::digest::sha256_hex;
use crate::exec_dag::ExecNodeKindV1;
use crate::schema::canonical_json;

/// The protected decision info a model saw at one node (ZS-EXEC-002: model
/// sees the same protected decision info as the primitive trace). Field
/// values are the offered question/choices, the observed value, and the
/// resolved alternative plus the policy rule that resolved it (None when
/// uncovered -- which aborts execution before a trace is settled).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedDecisionViewV1 {
    /// The question posed at the decision point.
    pub question: String,
    /// The offered alternatives (non-empty).
    pub choices: Vec<String>,
    /// The observed value (must be offered).
    pub observed_value: String,
    /// The resolved alternative (must be offered).
    pub resolved_alternative: String,
    /// Id of the policy rule that resolved the point, when one matched.
    pub policy_rule_id: Option<String>,
}

impl ProtectedDecisionViewV1 {
    /// Fail-closed validation: non-empty question, non-empty choices,
    /// observed and resolved values must be offered.
    pub fn validate(&self) -> Result<(), ExecTraceErrorV1> {
        if self.question.trim().is_empty() {
            return Err(ExecTraceErrorV1::InvalidDecisionView(
                "empty question".into(),
            ));
        }
        if self.choices.is_empty() {
            return Err(ExecTraceErrorV1::InvalidDecisionView(
                "empty choices".into(),
            ));
        }
        if !self.choices.iter().any(|c| c == &self.observed_value) {
            return Err(ExecTraceErrorV1::InvalidDecisionView(format!(
                "observed value {:?} not offered in choices",
                self.observed_value
            )));
        }
        if !self.choices.iter().any(|c| c == &self.resolved_alternative) {
            return Err(ExecTraceErrorV1::InvalidDecisionView(format!(
                "resolved alternative {:?} not offered in choices",
                self.resolved_alternative
            )));
        }
        if let Some(rule_id) = &self.policy_rule_id {
            if rule_id.trim().is_empty() {
                return Err(ExecTraceErrorV1::InvalidDecisionView(
                    "empty policy rule id".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Outcome of one traced node (settled executions only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum TraceOutcomeV1 {
    /// Node completed with a deterministic result digest.
    Completed { result_digest: String },
    /// Node failed with a typed failure code.
    Failed { failure_code: String },
}

/// One record of a settled execution, in deterministic topological order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecTraceRecordV1 {
    /// Node id.
    pub node_id: String,
    /// Node kind (plain op or decision boundary).
    pub kind: ExecNodeKindV1,
    /// Node outcome.
    pub outcome: TraceOutcomeV1,
    /// Protected decision info the model saw at this node, if any.
    pub protected_decision: Option<ProtectedDecisionViewV1>,
}

/// Deterministic execution trace of a plan DAG (ZS-EXEC-002/005).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecTraceV1 {
    /// Canonical digest of the executed plan (same plan => same digest).
    pub plan_digest: String,
    /// Digest of the execution inputs (same inputs => same digest).
    pub input_digest: String,
    /// Per-node records in deterministic topological order.
    pub records: Vec<ExecTraceRecordV1>,
}

impl ExecTraceV1 {
    /// New trace; records must already be in topological order (the
    /// executor guarantees this; `equivalence` compares order-sensitive).
    pub fn new(plan_digest: impl Into<String>, input_digest: impl Into<String>, records: Vec<ExecTraceRecordV1>) -> Self {
        ExecTraceV1 {
            plan_digest: plan_digest.into(),
            input_digest: input_digest.into(),
            records,
        }
    }

    /// Deterministic content root: SHA-256 over canonical JSON of the whole
    /// trace (plan digest + input digest + ordered records).
    pub fn trace_root(&self) -> String {
        let value = serde_json::to_value(self).expect("trace serializes");
        sha256_hex(canonical_json(&value).as_bytes())
    }

    /// Equivalence comparison: same plan + same inputs => equivalent trace.
    /// Any difference is reported loudly as a typed first-divergence point
    /// (record index, node id, field, expected/actual), never silently.
    pub fn equivalence(&self, other: &ExecTraceV1) -> TraceEquivalenceV1 {
        if self.plan_digest != other.plan_digest {
            return divergence(0, "", "plan_digest", &self.plan_digest, &other.plan_digest);
        }
        if self.input_digest != other.input_digest {
            return divergence(0, "", "input_digest", &self.input_digest, &other.input_digest);
        }
        if self.records.len() != other.records.len() {
            return divergence(
                self.records.len().min(other.records.len()),
                "",
                "record_count",
                &self.records.len(),
                &other.records.len(),
            );
        }
        for (index, (left, right)) in self.records.iter().zip(other.records.iter()).enumerate() {
            if left.node_id != right.node_id {
                return divergence(index, &left.node_id, "node_id", &left.node_id, &right.node_id);
            }
            if left.kind != right.kind {
                return divergence(index, &left.node_id, "kind", &left.kind, &right.kind);
            }
            if left.outcome != right.outcome {
                return divergence(index, &left.node_id, "outcome", &left.outcome, &right.outcome);
            }
            match (&left.protected_decision, &right.protected_decision) {
                (Some(a), Some(b)) => {
                    if a.question != b.question {
                        return divergence(
                            index,
                            &left.node_id,
                            "protected_decision.question",
                            &a.question,
                            &b.question,
                        );
                    }
                    if a.choices != b.choices {
                        return divergence(
                            index,
                            &left.node_id,
                            "protected_decision.choices",
                            &a.choices,
                            &b.choices,
                        );
                    }
                    if a.observed_value != b.observed_value {
                        return divergence(
                            index,
                            &left.node_id,
                            "protected_decision.observed_value",
                            &a.observed_value,
                            &b.observed_value,
                        );
                    }
                    if a.resolved_alternative != b.resolved_alternative {
                        return divergence(
                            index,
                            &left.node_id,
                            "protected_decision.resolved_alternative",
                            &a.resolved_alternative,
                            &b.resolved_alternative,
                        );
                    }
                    if a.policy_rule_id != b.policy_rule_id {
                        return divergence(
                            index,
                            &left.node_id,
                            "protected_decision.policy_rule_id",
                            &a.policy_rule_id,
                            &b.policy_rule_id,
                        );
                    }
                }
                (None, None) => {}
                _ => {
                    return divergence(
                        index,
                        &left.node_id,
                        "protected_decision",
                        &left.protected_decision,
                        &right.protected_decision,
                    );
                }
            }
        }
        TraceEquivalenceV1 {
            equivalent: true,
            first_divergence: None,
        }
    }
}

/// Build a typed first-divergence report.
fn divergence(
    record_index: usize,
    node_id: &str,
    field: &str,
    expected: &impl Serialize,
    actual: &impl Serialize,
) -> TraceEquivalenceV1 {
    TraceEquivalenceV1 {
        equivalent: false,
        first_divergence: Some(TraceDivergenceV1 {
            record_index,
            node_id: node_id.to_string(),
            field: field.to_string(),
            expected: serde_json::to_string(expected).expect("serializes"),
            actual: serde_json::to_string(actual).expect("serializes"),
        }),
    }
}

/// The first divergence between two traces, pinpointed (ZS-EXEC-005).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceDivergenceV1 {
    /// Index of the first diverging record (0 for root-level divergences).
    pub record_index: usize,
    /// Node id of the first diverging record ("" for root-level).
    pub node_id: String,
    /// Diverging field (e.g. `plan_digest`, `record_count`, `outcome`,
    /// `protected_decision.resolved_alternative`).
    pub field: String,
    /// Expected value (JSON text).
    pub expected: String,
    /// Actual value (JSON text).
    pub actual: String,
}

impl TraceDivergenceV1 {
    /// Loud human-readable description for logs.
    pub fn describe(&self) -> String {
        format!(
            "trace divergence at record {} (node {:?}): field '{}' expected {} actual {}",
            self.record_index, self.node_id, self.field, self.expected, self.actual
        )
    }
}

/// Result of an equivalence comparison: never a silent boolean.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEquivalenceV1 {
    /// True only when plan digest, input digest, and every record match.
    pub equivalent: bool,
    /// The first divergence, when not equivalent.
    pub first_divergence: Option<TraceDivergenceV1>,
}

/// Fail-closed trace errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecTraceErrorV1 {
    /// A protected decision view failed validation.
    InvalidDecisionView(String),
}

impl std::fmt::Display for ExecTraceErrorV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecTraceErrorV1::InvalidDecisionView(detail) => {
                write!(f, "invalid protected decision view: {detail}")
            }
        }
    }
}

impl std::error::Error for ExecTraceErrorV1 {}

