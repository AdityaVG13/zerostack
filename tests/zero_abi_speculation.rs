use std::collections::BTreeSet;

use zero_abi::{
    ExecDag, ExecNode, ExecNodeKind, FinalizedCallProof, SpeculationBinding, SpeculationCandidate,
    SpeculationPermit, SpeculativeOperation, compile_finalized_speculation_plan,
};

fn root(byte: char) -> String {
    std::iter::repeat_n(byte, 64).collect()
}

fn permit(occurrence: u32) -> SpeculationPermit {
    SpeculationPermit {
        node_id: "read:src".into(),
        operation: SpeculativeOperation::Read,
        arguments: serde_json::json!(["src/lib.rs"]),
        binding: SpeculationBinding {
            capsule_root: root('a'),
            state_root: root('b'),
            contract_root: root('c'),
            epoch: 1,
        },
        proof: FinalizedCallProof {
            finalized_source_root: root('d'),
            execution_dag_root: root('e'),
            node_root: root('f'),
            verifier_root: root('1'),
            exact_input_roots: vec![root('2')],
            unconditional: true,
        },
        occurrence,
        certified_pure: true,
        cancellation_bound: true,
        work_budget: 1,
        provider_token_budget: 0,
    }
}

#[test]
fn claim_identity_binds_occurrence_capsule_and_finalized_dag() {
    let base = permit(1);
    let base_key = base.claim_key().unwrap();
    // Changing only occurrence must change claim_key
    let mut p = base.clone();
    p.occurrence = 2;
    assert_ne!(p.claim_key().unwrap(), base_key);
    p.occurrence = 1;
    assert_eq!(p.claim_key().unwrap(), base_key);
    // Changing only capsule_root must change claim_key
    let mut p = base.clone();
    p.binding.capsule_root = root('9');
    assert_ne!(p.claim_key().unwrap(), base_key);
    p.binding.capsule_root = base.binding.capsule_root.clone();
    assert_eq!(p.claim_key().unwrap(), base_key);
    // Changing only finalized DAG node_root must change claim_key
    let mut p = base.clone();
    p.proof.node_root = root('8');
    assert_ne!(p.claim_key().unwrap(), base_key);
    p.proof.node_root = base.proof.node_root.clone();
    assert_eq!(p.claim_key().unwrap(), base_key);
}

#[test]
fn prediction_impurity_and_unbounded_work_are_never_admitted() {
    let baseline = permit(1);
    assert!(baseline.validate().is_ok(), "baseline permit must be valid");
    let mut impure = baseline.clone();
    impure.proof.unconditional = false;
    assert!(impure.validate().is_err());
    let mut uncertain = baseline.clone();
    uncertain.certified_pure = false;
    assert!(uncertain.validate().is_err());
    let mut unbounded = baseline.clone();
    unbounded.cancellation_bound = false;
    assert!(unbounded.validate().is_err());
}

#[test]
fn only_read_find_and_certified_extensions_are_speculatable() {
    assert_eq!(
        SpeculativeOperation::from_zero_method("read"),
        Some(SpeculativeOperation::Read)
    );
    assert_eq!(
        SpeculativeOperation::from_zero_method("find"),
        Some(SpeculativeOperation::Find)
    );
    for method in ["edit", "apply", "run", "state"] {
        assert_eq!(SpeculativeOperation::from_zero_method(method), None);
    }
}

#[test]
fn finalized_dag_compiler_admits_only_proven_unconditional_calls() {
    let dag = ExecDag::new(vec![
        ExecNode::new("read:src", ExecNodeKind::Op, 1, Vec::<String>::new()).unwrap(),
        ExecNode::new("find:symbol", ExecNodeKind::Op, 1, ["read:src"]).unwrap(),
    ]);
    let candidate = |node_id: &str, operation| SpeculationCandidate {
        node_id: node_id.into(),
        operation,
        arguments: serde_json::json!(["src/lib.rs"]),
        exact_input_roots: vec![root('5')],
        occurrence: 1,
        certified_pure: true,
        cancellation_bound: true,
        work_budget: 1,
        provider_token_budget: 0,
    };
    let plan = compile_finalized_speculation_plan(
        root('6'),
        SpeculationBinding {
            capsule_root: root('a'),
            state_root: root('b'),
            contract_root: root('c'),
            epoch: 1,
        },
        &dag,
        root('7'),
        &BTreeSet::from(["read:src".into()]),
        vec![
            candidate("read:src", SpeculativeOperation::Read),
            candidate("find:symbol", SpeculativeOperation::Find),
        ],
    )
    .unwrap();
    assert_eq!(plan.speculative.len(), 1);
    assert_eq!(plan.speculative[0].node_id, "read:src");
    assert_eq!(plan.ordinary_node_ids, vec!["find:symbol"]);
}
