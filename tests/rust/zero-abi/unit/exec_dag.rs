//! Unit tests for the plan DAG surface (V6-R15, ZS-EXEC-001): fail-closed
//! validation, deterministic topological order, batchable independent
//! layers, critical path, and the contingent-policy crossing rule.

use super::*;
use crate::decision::{ContingentPolicyV1, ContingentPolicyRuleV1, ObservationClassV1, ObservedMatchV1};

fn node(id: &str, weight: u64, deps: &[&str]) -> ExecNodeV1 {
    ExecNodeV1::new(id, ExecNodeKindV1::Op, weight, deps.iter().copied()).expect("valid node")
}

fn boundary(id: &str, deps: &[&str]) -> ExecNodeV1 {
    ExecNodeV1::new(id, ExecNodeKindV1::DecisionBoundary, 0, deps.iter().copied())
        .expect("valid boundary node")
}

fn rule(class_id: &str, alternative: &str) -> ContingentPolicyRuleV1 {
    ContingentPolicyRuleV1::new(
        ObservationClassV1::new(class_id).expect("valid class"),
        ObservedMatchV1::Exact { value: "fast".into() },
        alternative,
    )
    .expect("valid rule")
}

#[test]
fn validation_rejects_duplicate_ids() {
    let dag = ExecDagV1::new(vec![node("a", 1, &[]), node("a", 1, &[])]);
    assert_eq!(
        dag.validate(),
        Err(ExecDagErrorV1::DuplicateNodeId { id: "a".into() })
    );
}

#[test]
fn validation_rejects_missing_dependency() {
    let dag = ExecDagV1::new(vec![node("a", 1, &["ghost"])]);
    assert_eq!(
        dag.validate(),
        Err(ExecDagErrorV1::MissingDependency {
            node: "a".into(),
            dep: "ghost".into()
        })
    );
}

#[test]
fn new_rejects_self_dependency() {
    assert_eq!(
        ExecNodeV1::new("a", ExecNodeKindV1::Op, 1, ["a"]),
        Err(ExecDagErrorV1::SelfDependency { node: "a".into() })
    );
}

#[test]
fn validation_rejects_cycle_with_remaining_nodes() {
    let dag = ExecDagV1::new(vec![node("a", 1, &["b"]), node("b", 1, &["a"])]);
    let error = dag.validate().expect_err("cycle must fail");
    assert!(matches!(error, ExecDagErrorV1::CycleDetected { remaining } if remaining.len() == 2));
}

#[test]
fn topo_order_is_deterministic_and_respects_dependencies() {
    // Diamond: a -> b, a -> c, b -> d, c -> d. Ready-set sorted by id.
    let dag = ExecDagV1::new(vec![
        node("d", 1, &["b", "c"]),
        node("c", 1, &["a"]),
        node("b", 1, &["a"]),
        node("a", 1, &[]),
    ]);
    let order = dag.topo_order().expect("valid dag");
    assert_eq!(order, vec!["a", "b", "c", "d"]);
    // Deterministic across repeated calls.
    assert_eq!(order, dag.topo_order().expect("valid dag"));
}

#[test]
fn independent_nodes_form_batchable_layers() {
    // a -> b, a -> c, b -> d, c -> d
    let dag = ExecDagV1::new(vec![
        node("d", 1, &["b", "c"]),
        node("c", 1, &["a"]),
        node("b", 1, &["a"]),
        node("a", 1, &[]),
    ]);
    let layers = dag.layers().expect("valid dag");
    assert_eq!(layers, vec![vec!["a"], vec!["b", "c"], vec!["d"]]);
    // b and c are mutually independent: batchable in one layer.
    assert_eq!(layers[1].len(), 2);
}

#[test]
fn critical_path_uses_weights_with_deterministic_tie_break() {
    // Two chains into one sink: a -> b (weight 100), c -> d (weight 1).
    let dag = ExecDagV1::new(vec![
        node("a", 100, &[]),
        node("b", 1, &["a"]),
        node("c", 1, &[]),
        node("d", 1, &["c"]),
    ]);
    assert_eq!(dag.critical_path().expect("valid dag"), vec!["a", "b"]);
}

#[test]
fn critical_path_tie_breaks_to_smallest_id() {
    // Two independent equal-weight nodes; the path is the heavier end node
    // with lexicographically smallest id.
    let dag = ExecDagV1::new(vec![node("z", 1, &[]), node("a", 1, &[])]);
    assert_eq!(dag.critical_path().expect("valid dag"), vec!["a"]);
}

#[test]
fn empty_dag_is_valid_with_empty_order_and_path() {
    let dag = ExecDagV1::new(vec![]);
    assert!(dag.validate().is_ok());
    assert!(dag.topo_order().expect("ok").is_empty());
    assert!(dag.critical_path().expect("ok").is_empty());
}

#[test]
fn crossing_rule_fails_closed_without_policy() {
    let dag = ExecDagV1::new(vec![node("a", 1, &[]), boundary("dec:1", &["a"])]);
    assert!(dag.requires_policy());
    assert_eq!(
        dag.crossing_rule(None),
        Err(ExecDagErrorV1::DecisionBoundaryUncovered {
            node_id: "dec:1".into()
        })
    );
}

#[test]
fn crossing_rule_names_first_boundary_in_topo_order() {
    let dag = ExecDagV1::new(vec![
        boundary("dec:z", &[]),
        boundary("dec:a", &[]),
    ]);
    assert_eq!(
        dag.crossing_rule(None),
        Err(ExecDagErrorV1::DecisionBoundaryUncovered {
            node_id: "dec:a".into()
        })
    );
}

#[test]
fn crossing_rule_permits_with_valid_policy_and_rejects_defective_one() {
    let dag = ExecDagV1::new(vec![boundary("dec:1", &[])]);
    let policy = ContingentPolicyV1::new(vec![rule("branch.test_suite", "run_fast")])
        .expect("valid policy");
    assert!(dag.crossing_rule(Some(&policy)).is_ok());
    // Defective policy (empty select_alternative) is refused, not silently
    // ignored.
    let defective = ContingentPolicyV1 {
        rules: vec![ContingentPolicyRuleV1 {
            observation_class: ObservationClassV1::new("branch.test_suite").expect("class"),
            observed: ObservedMatchV1::Any,
            select_alternative: "".into(),
        }],
    };
    assert!(dag.crossing_rule(Some(&defective)).is_err());
}

#[test]
fn plan_without_boundaries_needs_no_policy() {
    let dag = ExecDagV1::new(vec![node("a", 1, &[])]);
    assert!(!dag.requires_policy());
    assert!(dag.crossing_rule(None).is_ok());
}

#[test]
fn plan_digest_is_canonical_across_insertion_order() {
    let left = ExecDagV1::new(vec![node("a", 1, &[]), node("b", 2, &["a"])]);
    let right = ExecDagV1::new(vec![node("b", 2, &["a"]), node("a", 1, &[])]);
    assert_eq!(
        left.plan_digest().expect("valid"),
        right.plan_digest().expect("valid")
    );
    let changed = ExecDagV1::new(vec![node("a", 1, &[]), node("b", 3, &["a"])]);
    assert_ne!(
        left.plan_digest().expect("valid"),
        changed.plan_digest().expect("valid")
    );
}

#[test]
fn dag_round_trips_through_json() {
    let dag = ExecDagV1::new(vec![node("a", 1, &[]), boundary("dec:1", &["a"])]);
    let json = serde_json::to_value(&dag).expect("serializes");
    let decoded: ExecDagV1 = serde_json::from_value(json).expect("deserializes");
    assert_eq!(decoded, dag);
    assert!(decoded.validate().is_ok());
}
