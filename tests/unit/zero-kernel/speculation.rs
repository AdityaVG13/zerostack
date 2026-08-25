use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::Value;
use zero_abi::{
    EngineErrorKind, ExecDag, ExecNode, ExecNodeKind, FinalizedCallProof, SpeculationAdmission,
    SpeculationBinding, SpeculationCandidate, SpeculationPermit, SpeculativeOperation,
};
use zero_kernel::{CellPreparation, SpeculationRuntime};

fn root(byte: char) -> String {
    std::iter::repeat_n(byte, 64).collect()
}

fn permit(occurrence: u32) -> SpeculationPermit {
    SpeculationPermit {
        node_id: format!("read:{occurrence}"),
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
        work_budget: 3,
        provider_token_budget: 17,
    }
}

#[test]
fn admitted_call_has_one_execution_and_one_exact_claim() {
    let runtime = SpeculationRuntime::new(2).unwrap();
    let permit = permit(1);
    let admission = runtime
        .admit(permit.clone(), |_| Ok(serde_json::json!("speculated")))
        .unwrap();
    assert_eq!(admission, SpeculationAdmission::Speculated);

    let value = runtime.claim(&permit, Duration::from_secs(1)).unwrap();
    assert_eq!(value, serde_json::json!("speculated"));
    let ledger = runtime.end_turn().unwrap();
    assert_eq!(ledger.dispatched, 1);
    assert_eq!(ledger.claim_hits, 1);
    assert_eq!(ledger.claim_invariant_failures, 0);
    assert_eq!(ledger.provider_tokens_dispatched, 17);
    assert_eq!(ledger.provider_tokens_claimed, 17);
    assert_eq!(ledger.provider_tokens_wasted_upper_bound, 0);
    assert_eq!(ledger.work_units_dispatched, ledger.work_units_claimed);
}

#[test]
fn absent_exact_claim_is_an_invariant_failure_not_a_retry() {
    let runtime = SpeculationRuntime::new(1).unwrap();
    let error = runtime
        .claim(&permit(1), Duration::from_millis(1))
        .unwrap_err();
    assert_eq!(error.kind, EngineErrorKind::Internal);
    assert_eq!(runtime.ledger().claim_invariant_failures, 1);
    assert_eq!(runtime.ledger().dispatched, 0);
}

#[test]
fn capacity_refusal_selects_ordinary_before_second_work_launches() {
    let runtime = SpeculationRuntime::new(1).unwrap();
    runtime
        .admit(permit(1), |cancellation| {
            while !cancellation.is_cancelled() {
                std::thread::yield_now();
            }
            Ok(Value::Null)
        })
        .unwrap();
    let admission = runtime
        .admit(permit(2), |_| {
            panic!("ordinary admission must not launch work")
        })
        .unwrap();
    assert_eq!(admission, SpeculationAdmission::Ordinary);
    let ledger = runtime.end_turn().unwrap();
    assert_eq!(ledger.ordinary_admissions, 1);
    assert_eq!(ledger.cancelled, 1);
}

#[test]
fn turn_invalidation_cancels_and_joins_unclaimed_work() {
    let runtime = SpeculationRuntime::new(1).unwrap();
    runtime
        .admit(permit(1), |cancellation| {
            while !cancellation.is_cancelled() {
                std::thread::yield_now();
            }
            Ok(Value::Null)
        })
        .unwrap();
    let ledger = runtime.end_turn().unwrap();
    assert_eq!(ledger.cancelled, 1);
    assert_eq!(ledger.claim_hits, 0);
    assert_eq!(ledger.provider_tokens_wasted_upper_bound, 17);
}

#[test]
fn finalized_preparation_compiles_rooted_zero_miss_plan() {
    let mut preparation = CellPreparation::new();
    preparation.feed("const snap = await ").unwrap();
    preparation.feed("z.read(\"src/lib.rs\");").unwrap();
    let prepared = preparation.finish().unwrap();
    assert_eq!(
        prepared.source(),
        "const snap = await z.read(\"src/lib.rs\");"
    );
    assert_eq!(
        prepared.digest(),
        blake3::hash(prepared.source().as_bytes()).to_hex().as_str()
    );

    let dag = ExecDag::new(vec![
        ExecNode::new("read:src", ExecNodeKind::Op, 1, Vec::<String>::new()).unwrap(),
    ]);
    let plan = prepared
        .compile_speculation_plan(
            SpeculationBinding {
                capsule_root: root('a'),
                state_root: root('b'),
                contract_root: root('c'),
                epoch: 1,
            },
            &dag,
            root('3'),
            &BTreeSet::from(["read:src".into()]),
            vec![SpeculationCandidate {
                node_id: "read:src".into(),
                operation: SpeculativeOperation::Read,
                arguments: serde_json::json!(["src/lib.rs"]),
                exact_input_roots: vec![root('4')],
                occurrence: 1,
                certified_pure: true,
                cancellation_bound: true,
                work_budget: 1,
                provider_token_budget: 0,
            }],
        )
        .unwrap();
    assert_eq!(plan.speculative.len(), 1);
    assert!(plan.ordinary_node_ids.is_empty());
}
