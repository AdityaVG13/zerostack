//! Unit tests for trace equivalence (V6-R15, ZS-EXEC-002/005): identical
//! replay is equivalent; every injected divergence is pinpointed loudly
//! (record index, node id, field, expected/actual).

use super::*;
use crate::exec_dag::ExecNodeKindV1;

fn view(alternative: &str) -> ProtectedDecisionViewV1 {
    ProtectedDecisionViewV1 {
        question: "which strategy?".into(),
        choices: vec!["fast".into(), "full".into()],
        observed_value: "fast".into(),
        resolved_alternative: alternative.into(),
        policy_rule_id: Some("rule:1".into()),
    }
}

fn record(node_id: &str, digest: &str, decision: Option<ProtectedDecisionViewV1>) -> ExecTraceRecordV1 {
    ExecTraceRecordV1 {
        node_id: node_id.into(),
        kind: ExecNodeKindV1::Op,
        outcome: TraceOutcomeV1::Completed {
            result_digest: digest.into(),
        },
        protected_decision: decision,
    }
}

fn trace(records: Vec<ExecTraceRecordV1>) -> ExecTraceV1 {
    ExecTraceV1::new("plan:abc", "input:123", records)
}

#[test]
fn identical_replay_is_equivalent() {
    let left = trace(vec![
        record("a", "d1", None),
        record("dec:1", "d2", Some(view("fast"))),
    ]);
    let right = trace(vec![
        record("a", "d1", None),
        record("dec:1", "d2", Some(view("fast"))),
    ]);
    let equivalence = left.equivalence(&right);
    assert!(equivalence.equivalent);
    assert!(equivalence.first_divergence.is_none());
}

#[test]
fn equivalence_is_symmetric_for_identical_traces() {
    let left = trace(vec![record("a", "d1", None)]);
    let right = left.clone();
    assert_eq!(left.equivalence(&right), right.equivalence(&left));
}

#[test]
fn injected_result_divergence_is_pinpointed() {
    let left = trace(vec![record("a", "d1", None), record("b", "d2", None)]);
    let right = trace(vec![record("a", "d1", None), record("b", "d2-injected", None)]);
    let divergence = left
        .equivalence(&right)
        .first_divergence
        .expect("divergence must be reported");
    assert_eq!(divergence.record_index, 1);
    assert_eq!(divergence.node_id, "b");
    assert_eq!(divergence.field, "outcome");
    assert!(divergence.expected.contains("d2"));
    assert!(divergence.actual.contains("d2-injected"));
    assert!(divergence.describe().contains("record 1"));
}

#[test]
fn injected_protected_decision_divergence_is_pinpointed() {
    let left = trace(vec![record("dec:1", "d", Some(view("fast")))]);
    let right = trace(vec![record("dec:1", "d", Some(view("full")))]);
    let divergence = left
        .equivalence(&right)
        .first_divergence
        .expect("divergence must be reported");
    assert_eq!(divergence.record_index, 0);
    assert_eq!(divergence.node_id, "dec:1");
    assert_eq!(divergence.field, "protected_decision.resolved_alternative");
}

#[test]
fn missing_protected_decision_diverges() {
    let left = trace(vec![record("dec:1", "d", Some(view("fast")))]);
    let right = trace(vec![record("dec:1", "d", None)]);
    let divergence = left
        .equivalence(&right)
        .first_divergence
        .expect("divergence must be reported");
    assert_eq!(divergence.field, "protected_decision");
}

#[test]
fn plan_digest_mismatch_diverges_at_root() {
    let left = trace(vec![record("a", "d", None)]);
    let mut right = left.clone();
    right.plan_digest = "plan:other".into();
    let divergence = left
        .equivalence(&right)
        .first_divergence
        .expect("divergence must be reported");
    assert_eq!(divergence.record_index, 0);
    assert_eq!(divergence.node_id, "");
    assert_eq!(divergence.field, "plan_digest");
}

#[test]
fn input_digest_mismatch_diverges() {
    let left = trace(vec![record("a", "d", None)]);
    let mut right = left.clone();
    right.input_digest = "input:other".into();
    let divergence = left
        .equivalence(&right)
        .first_divergence
        .expect("divergence must be reported");
    assert_eq!(divergence.field, "input_digest");
}

#[test]
fn record_count_mismatch_diverges() {
    let left = trace(vec![record("a", "d1", None), record("b", "d2", None)]);
    let right = trace(vec![record("a", "d1", None)]);
    let divergence = left
        .equivalence(&right)
        .first_divergence
        .expect("divergence must be reported");
    assert_eq!(divergence.field, "record_count");
    assert_eq!(divergence.record_index, 1);
}

#[test]
fn trace_root_is_deterministic_and_sensitive() {
    let t = trace(vec![record("a", "d1", None)]);
    assert_eq!(t.trace_root(), t.trace_root());
    let changed = trace(vec![record("a", "d2", None)]);
    assert_ne!(t.trace_root(), changed.trace_root());
}

#[test]
fn protected_decision_view_validation_is_fail_closed() {
    let mut v = view("fast");
    assert!(v.validate().is_ok());
    v.resolved_alternative = "not-offered".into();
    assert!(v.validate().is_err());
    v = view("fast");
    v.observed_value = "not-offered".into();
    assert!(v.validate().is_err());
    v = view("fast");
    v.choices = vec![];
    assert!(v.validate().is_err());
}

#[test]
fn trace_round_trips_through_json() {
    let trace = trace(vec![
        record("a", "d1", None),
        record("dec:1", "d2", Some(view("fast"))),
    ]);
    let json = serde_json::to_value(&trace).expect("serializes");
    let decoded: ExecTraceV1 = serde_json::from_value(json).expect("deserializes");
    assert_eq!(decoded, trace);
}
