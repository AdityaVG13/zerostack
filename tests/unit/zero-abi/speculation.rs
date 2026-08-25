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
    let first = permit(1);
    let mut second = permit(2);
    assert_ne!(first.claim_key().unwrap(), second.claim_key().unwrap());

    second.occurrence = 1;
    second.binding.state_root = root('3');
    assert_ne!(first.claim_key().unwrap(), second.claim_key().unwrap());

    second.binding.state_root = first.binding.state_root.clone();
    second.proof.node_root = root('4');
    assert_ne!(first.claim_key().unwrap(), second.claim_key().unwrap());
}

#[test]
fn prediction_impurity_and_unbounded_work_are_never_admitted() {
    let mut candidate = permit(1);
    candidate.proof.unconditional = false;
    assert!(candidate.validate().is_err());

    candidate.proof.unconditional = true;
    candidate.certified_pure = false;
    assert!(candidate.validate().is_err());

    candidate.certified_pure = true;
    candidate.cancellation_bound = false;
    assert!(candidate.validate().is_err());
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
